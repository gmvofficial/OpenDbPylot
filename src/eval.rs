//! Accuracy measurement.
//!
//! # Why this exists
//!
//! The claim "accurate NL→SQL" is unfalsifiable without a number, and the
//! previous harness — twenty hand-written questions against one six-table demo
//! database, five of them the exact question/SQL pairs the retriever had been
//! trained on — measured plumbing rather than capability. You could not tell
//! from it whether a change helped, and there was nothing a CI run could diff.
//!
//! This module makes accuracy a measurement:
//!
//! - **Execution accuracy**, not string comparison. The generated SQL and the
//!   reference SQL are both run and their result sets compared, so a correct
//!   query written differently still counts as correct.
//! - **Held-out marking.** Every case says whether the model was trained on it.
//!   A score that mixes seen and unseen cases overstates capability, so the
//!   scorecard reports them separately and leads with the unseen number.
//! - **Difficulty buckets**, so a regression can be attributed rather than just
//!   noticed.
//! - **A JSON scorecard**, so one commit's run can be compared against another's.
//!
//! # Case format
//!
//! Spider-shaped, which is what public text-to-SQL benchmarks publish:
//!
//! ```json
//! [{"question": "how many orders?", "query": "SELECT COUNT(*) FROM orders;",
//!   "difficulty": "easy", "seen": false}]
//! ```
//!
//! `db_id` is accepted and ignored — a run targets one connected database.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::opendbpylot::OpenDbPylot;
use crate::sqlrunner::{QueryResult, SqlRunner};

/// One benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Case {
    pub question: String,
    /// Reference SQL, hand-checked against the schema.
    #[serde(alias = "sql")]
    pub query: String,
    /// `easy` | `medium` | `hard`. Free-form; used only for bucketing.
    #[serde(default = "default_difficulty")]
    pub difficulty: String,
    /// Whether this exact question was part of the training corpus.
    ///
    /// Defaults to false — assuming a case is unseen is the conservative
    /// direction, since counting a seen case as unseen can only understate the
    /// score.
    #[serde(default)]
    pub seen: bool,
    /// Ignored; accepted so Spider files load unmodified.
    #[serde(default)]
    pub db_id: Option<String>,
}

fn default_difficulty() -> String {
    "unspecified".to_string()
}

/// How one case came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Rows matched exactly, in column order.
    Exact,
    /// Rows matched after sorting cells within each row — tolerates a different
    /// column order or an extra alias carrying the same values.
    Equivalent,
    /// Ran, but produced different rows.
    Wrong,
    /// The generated SQL would not run, even after repair attempts.
    Failed,
    /// The reference SQL is broken, so the case cannot be scored. Counted
    /// separately: a bad reference is the harness's fault, not the model's.
    BadReference,
}

impl Verdict {
    /// Whether this counts toward accuracy.
    pub fn is_correct(self) -> bool {
        matches!(self, Verdict::Exact | Verdict::Equivalent)
    }

    /// Whether the case was scoreable at all.
    pub fn is_scored(self) -> bool {
        !matches!(self, Verdict::BadReference)
    }
}

/// What happened on one case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub question: String,
    pub difficulty: String,
    pub seen: bool,
    pub verdict: Verdict,
    pub repairs_used: usize,
    /// The SQL the model produced, kept for failures so a run is debuggable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_sql: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub elapsed_ms: u128,
}

/// Accuracy over some subset of cases.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Score {
    pub total: usize,
    pub exact: usize,
    pub equivalent: usize,
    pub wrong: usize,
    pub failed: usize,
}

impl Score {
    fn record(&mut self, verdict: Verdict) {
        self.total += 1;
        match verdict {
            Verdict::Exact => self.exact += 1,
            Verdict::Equivalent => self.equivalent += 1,
            Verdict::Wrong => self.wrong += 1,
            Verdict::Failed => self.failed += 1,
            Verdict::BadReference => self.total -= 1,
        }
    }

    /// Correct cases as a fraction of scoreable ones. An empty set scores 0,
    /// never 1 — "nothing ran" must not read as "everything passed".
    pub fn accuracy(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        (self.exact + self.equivalent) as f64 / self.total as f64
    }

    /// Exact-match accuracy alone, ignoring the lenient comparison.
    pub fn exact_accuracy(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.exact as f64 / self.total as f64
    }
}

/// Accuracy across repeated runs of the same cases.
///
/// # Why repeats are not optional
///
/// The model is nondeterministic, so one run measures one sample. Three runs
/// of this harness on an unchanged pipeline produced 75%, 65% and 60% on the
/// same twenty cases — a spread of fifteen points, which is wider than almost
/// any change worth making. A single number cannot tell a real regression from
/// that noise, so a comparison that matters reports the spread alongside the
/// mean and says whether the difference clears it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repeated {
    pub runs: usize,
    /// Held-out accuracy from each run, in order.
    pub accuracies: Vec<f64>,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    /// Population standard deviation.
    pub std_dev: f64,
}

impl Repeated {
    pub fn from_runs(accuracies: Vec<f64>) -> Self {
        let runs = accuracies.len();
        if runs == 0 {
            return Self {
                runs: 0,
                accuracies,
                mean: 0.0,
                min: 0.0,
                max: 0.0,
                std_dev: 0.0,
            };
        }
        let mean = accuracies.iter().sum::<f64>() / runs as f64;
        let variance =
            accuracies.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / runs as f64;

        Self {
            runs,
            min: accuracies.iter().cloned().fold(f64::INFINITY, f64::min),
            max: accuracies.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
            mean,
            std_dev: variance.sqrt(),
            accuracies,
        }
    }

    /// The spread, in percentage points. A change smaller than this is not
    /// distinguishable from noise at this sample size.
    pub fn spread_points(&self) -> f64 {
        (self.max - self.min) * 100.0
    }

    /// Whether a difference of `points` against this baseline is larger than
    /// the observed noise.
    ///
    /// Deliberately strict: with a handful of runs the spread is itself a rough
    /// estimate, so anything inside it is reported as inconclusive rather than
    /// as an improvement.
    pub fn is_significant(&self, points: f64) -> bool {
        points.abs() > self.spread_points()
    }

    pub fn summary(&self) -> String {
        if self.runs <= 1 {
            return format!(
                "held-out {:.1}% (1 run — too few to separate a change from noise)\n",
                self.mean * 100.0
            );
        }
        format!(
            "held-out {:.1}% mean over {} runs (min {:.1}%, max {:.1}%, spread {:.1} points)\n\
             A change smaller than {:.1} points is not distinguishable from noise here.\n",
            self.mean * 100.0,
            self.runs,
            self.min * 100.0,
            self.max * 100.0,
            self.spread_points(),
            self.spread_points(),
        )
    }
}

/// The whole run, as written to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scorecard {
    /// ISO-8601, so two scorecards can be ordered.
    pub run_at: String,
    pub model: String,
    pub dialect: String,
    /// Accuracy over cases the model was NOT trained on. The headline number:
    /// this is the one that says whether it can answer something new.
    pub held_out: Score,
    /// Accuracy over cases that were in the training corpus. Useful as a
    /// retrieval check, misleading as a capability claim.
    pub seen: Score,
    /// Everything together.
    pub overall: Score,
    pub by_difficulty: BTreeMap<String, Score>,
    /// Cases whose reference SQL would not run.
    pub bad_references: usize,
    pub total_elapsed_ms: u128,
    pub cases: Vec<CaseResult>,
    /// Set when the cases were run more than once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeated: Option<Repeated>,
}

impl Scorecard {
    /// A short human-readable summary.
    pub fn summary(&self) -> String {
        let mut out = format!(
            "held-out {:.1}%  ({}/{})\n",
            self.held_out.accuracy() * 100.0,
            self.held_out.exact + self.held_out.equivalent,
            self.held_out.total,
        );
        if self.seen.total > 0 {
            out.push_str(&format!(
                "seen     {:.1}%  ({}/{})\n",
                self.seen.accuracy() * 100.0,
                self.seen.exact + self.seen.equivalent,
                self.seen.total,
            ));
        }
        out.push_str(&format!(
            "overall  {:.1}%  ({}/{}), exact {:.1}%\n",
            self.overall.accuracy() * 100.0,
            self.overall.exact + self.overall.equivalent,
            self.overall.total,
            self.overall.exact_accuracy() * 100.0,
        ));
        for (difficulty, score) in &self.by_difficulty {
            out.push_str(&format!(
                "  {difficulty:<12} {:.1}%  ({}/{})\n",
                score.accuracy() * 100.0,
                score.exact + score.equivalent,
                score.total,
            ));
        }
        if self.bad_references > 0 {
            out.push_str(&format!(
                "\n{} case(s) had broken reference SQL and were not scored.\n",
                self.bad_references
            ));
        }
        out
    }

    /// Difference in held-out accuracy against an earlier run, in percentage
    /// points. Positive means this run is better.
    pub fn regression_against(&self, baseline: &Scorecard) -> f64 {
        (self.held_out.accuracy() - baseline.held_out.accuracy()) * 100.0
    }
}

/// Load cases from a JSON file.
pub fn load_cases(path: &Path) -> Result<Vec<Case>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("could not read cases from {}", path.display()))?;
    let cases: Vec<Case> = serde_json::from_str(&text)
        .with_context(|| format!("could not parse cases in {}", path.display()))?;
    if cases.is_empty() {
        anyhow::bail!("{} contains no cases", path.display());
    }
    Ok(cases)
}

/// Run every case and build a scorecard.
///
/// `auto_train` must be off on the engine passed in, or a passing case teaches
/// itself to later ones and the score stops being reproducible.
pub async fn run(
    engine: &OpenDbPylot,
    db: Arc<dyn SqlRunner>,
    cases: &[Case],
    model: &str,
    dialect: &str,
    mut on_case: impl FnMut(&CaseResult),
) -> Result<Scorecard> {
    let started = std::time::Instant::now();
    let mut results = Vec::with_capacity(cases.len());

    for case in cases {
        let case_started = std::time::Instant::now();

        // The reference is run first: if it is broken, the case tells us
        // nothing about the model and must not count against it.
        let expected = match db.run_sql(&case.query).await {
            Ok(rows) => rows,
            Err(e) => {
                let result = CaseResult {
                    question: case.question.clone(),
                    difficulty: case.difficulty.clone(),
                    seen: case.seen,
                    verdict: Verdict::BadReference,
                    repairs_used: 0,
                    generated_sql: None,
                    error: Some(format!("reference SQL failed: {e}")),
                    elapsed_ms: case_started.elapsed().as_millis(),
                };
                on_case(&result);
                results.push(result);
                continue;
            }
        };

        let result = match engine.ask(&case.question).await {
            Ok(answer) => {
                let verdict = match &answer.result {
                    Some(got) => compare(got, &expected),
                    // The model answered in prose instead of SQL.
                    None => Verdict::Failed,
                };
                CaseResult {
                    question: case.question.clone(),
                    difficulty: case.difficulty.clone(),
                    seen: case.seen,
                    verdict,
                    repairs_used: answer.repairs_used,
                    generated_sql: (!verdict.is_correct()).then(|| answer.sql.clone()),
                    error: None,
                    elapsed_ms: case_started.elapsed().as_millis(),
                }
            }
            Err(e) => CaseResult {
                question: case.question.clone(),
                difficulty: case.difficulty.clone(),
                seen: case.seen,
                verdict: Verdict::Failed,
                repairs_used: 0,
                generated_sql: None,
                error: Some(format!("{e:#}")),
                elapsed_ms: case_started.elapsed().as_millis(),
            },
        };

        on_case(&result);
        results.push(result);
    }

    Ok(build_scorecard(results, model, dialect, started.elapsed().as_millis()))
}

/// Aggregate case results into a scorecard.
pub fn build_scorecard(
    results: Vec<CaseResult>,
    model: &str,
    dialect: &str,
    total_elapsed_ms: u128,
) -> Scorecard {
    let mut held_out = Score::default();
    let mut seen = Score::default();
    let mut overall = Score::default();
    let mut by_difficulty: BTreeMap<String, Score> = BTreeMap::new();
    let mut bad_references = 0usize;

    for result in &results {
        if !result.verdict.is_scored() {
            bad_references += 1;
            continue;
        }
        overall.record(result.verdict);
        if result.seen {
            seen.record(result.verdict);
        } else {
            held_out.record(result.verdict);
        }
        by_difficulty
            .entry(result.difficulty.clone())
            .or_default()
            .record(result.verdict);
    }

    Scorecard {
        run_at: chrono_now(),
        model: model.to_string(),
        dialect: dialect.to_string(),
        held_out,
        seen,
        overall,
        by_difficulty,
        bad_references,
        total_elapsed_ms,
        cases: results,
        repeated: None,
    }
}

/// Compare a generated result against the reference.
pub fn compare(got: &QueryResult, expected: &QueryResult) -> Verdict {
    if normalise(got) == normalise(expected) {
        return Verdict::Exact;
    }
    if normalise_lenient(got) == normalise_lenient(expected) {
        return Verdict::Equivalent;
    }
    Verdict::Wrong
}

/// Normalise one cell so `1234.5`, `1234.50` and `1234.499` compare equal.
///
/// Without this, every aggregate query fails on floating-point formatting
/// rather than on being wrong.
fn normalise_cell(cell: &str) -> String {
    match cell.trim().parse::<f64>() {
        Ok(n) => format!("{n:.2}"),
        Err(_) => cell.trim().to_string(),
    }
}

/// Row multiset, cells in column order. Row order is ignored: `ORDER BY` is
/// rarely what the question was about.
fn normalise(result: &QueryResult) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| row.iter().map(|c| normalise_cell(c)).collect())
        .collect();
    rows.sort();
    rows
}

/// As `normalise`, but cells are sorted within each row too, so a different
/// column order still matches. More forgiving, and reported separately because
/// it can produce false positives.
fn normalise_lenient(result: &QueryResult) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .map(|row| {
            let mut cells: Vec<String> = row.iter().map(|c| normalise_cell(c)).collect();
            cells.sort();
            cells
        })
        .collect();
    rows.sort();
    rows
}

fn chrono_now() -> String {
    // Avoid a chrono dependency here: seconds since the epoch is enough to
    // order two scorecards, which is all this field is for.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(rows: Vec<Vec<&str>>) -> QueryResult {
        QueryResult {
            columns: vec!["a".into(), "b".into()],
            rows: rows
                .into_iter()
                .map(|r| r.into_iter().map(String::from).collect())
                .collect(),
        }
    }

    fn case_result(verdict: Verdict, seen: bool, difficulty: &str) -> CaseResult {
        CaseResult {
            question: "q".into(),
            difficulty: difficulty.into(),
            seen,
            verdict,
            repairs_used: 0,
            generated_sql: None,
            error: None,
            elapsed_ms: 0,
        }
    }

    // ── Comparison ───────────────────────────────────────────────────

    #[test]
    fn identical_results_are_exact() {
        let a = result(vec![vec!["1", "x"]]);
        let b = result(vec![vec!["1", "x"]]);
        assert_eq!(compare(&a, &b), Verdict::Exact);
    }

    #[test]
    fn row_order_does_not_matter() {
        // The question was almost never about ORDER BY.
        let a = result(vec![vec!["1", "x"], vec!["2", "y"]]);
        let b = result(vec![vec!["2", "y"], vec!["1", "x"]]);
        assert_eq!(compare(&a, &b), Verdict::Exact);
    }

    #[test]
    fn numeric_formatting_does_not_matter() {
        // Otherwise every SUM() fails on float formatting rather than on being
        // wrong, which would make the benchmark useless.
        let a = result(vec![vec!["1234.5", "x"]]);
        let b = result(vec![vec!["1234.50", "x"]]);
        assert_eq!(compare(&a, &b), Verdict::Exact);

        let c = result(vec![vec!["1234.499999", "x"]]);
        assert_eq!(compare(&a, &c), Verdict::Exact);
    }

    #[test]
    fn column_order_differences_are_equivalent_not_exact() {
        let a = result(vec![vec!["1", "x"]]);
        let b = result(vec![vec!["x", "1"]]);
        assert_eq!(compare(&a, &b), Verdict::Equivalent);
    }

    #[test]
    fn genuinely_different_rows_are_wrong() {
        let a = result(vec![vec!["1", "x"]]);
        let b = result(vec![vec!["2", "y"]]);
        assert_eq!(compare(&a, &b), Verdict::Wrong);
    }

    #[test]
    fn a_different_row_count_is_wrong() {
        let a = result(vec![vec!["1", "x"]]);
        let b = result(vec![vec!["1", "x"], vec!["1", "x"]]);
        assert_eq!(compare(&a, &b), Verdict::Wrong);
    }

    #[test]
    fn two_empty_results_match() {
        assert_eq!(compare(&result(vec![]), &result(vec![])), Verdict::Exact);
    }

    #[test]
    fn an_empty_result_does_not_match_a_populated_one() {
        // The failure mode this guards: a query returning nothing looking like
        // a pass against a reference that also happened to be compared loosely.
        assert_eq!(
            compare(&result(vec![]), &result(vec![vec!["1", "x"]])),
            Verdict::Wrong
        );
    }

    #[test]
    fn whitespace_around_a_value_does_not_matter() {
        let a = result(vec![vec![" x ", "1"]]);
        let b = result(vec![vec!["x", "1"]]);
        assert_eq!(compare(&a, &b), Verdict::Exact);
    }

    // ── Scoring ──────────────────────────────────────────────────────

    #[test]
    fn accuracy_counts_exact_and_equivalent() {
        let mut score = Score::default();
        score.record(Verdict::Exact);
        score.record(Verdict::Equivalent);
        score.record(Verdict::Wrong);
        score.record(Verdict::Failed);

        assert_eq!(score.total, 4);
        assert!((score.accuracy() - 0.5).abs() < 1e-9);
        assert!((score.exact_accuracy() - 0.25).abs() < 1e-9);
    }

    #[test]
    fn an_empty_score_is_zero_not_one() {
        // "Nothing ran" must never read as "everything passed".
        assert_eq!(Score::default().accuracy(), 0.0);
        assert_eq!(Score::default().exact_accuracy(), 0.0);
    }

    #[test]
    fn a_broken_reference_does_not_count_against_the_model() {
        let mut score = Score::default();
        score.record(Verdict::Exact);
        score.record(Verdict::BadReference);
        assert_eq!(score.total, 1, "the unscoreable case is excluded entirely");
        assert_eq!(score.accuracy(), 1.0);
    }

    #[test]
    fn verdicts_classify_themselves_consistently() {
        assert!(Verdict::Exact.is_correct());
        assert!(Verdict::Equivalent.is_correct());
        assert!(!Verdict::Wrong.is_correct());
        assert!(!Verdict::Failed.is_correct());
        assert!(!Verdict::BadReference.is_correct());

        assert!(Verdict::Wrong.is_scored());
        assert!(!Verdict::BadReference.is_scored());
    }

    // ── Scorecard ────────────────────────────────────────────────────

    #[test]
    fn seen_and_held_out_cases_are_scored_separately() {
        // Mixing them overstates capability, which is exactly what the old
        // harness did by including five trained-on questions in its twenty.
        let card = build_scorecard(
            vec![
                case_result(Verdict::Exact, true, "easy"),
                case_result(Verdict::Exact, true, "easy"),
                case_result(Verdict::Wrong, false, "hard"),
                case_result(Verdict::Exact, false, "hard"),
            ],
            "test-model",
            "SQLite",
            0,
        );

        assert_eq!(card.seen.total, 2);
        assert_eq!(card.seen.accuracy(), 1.0);
        assert_eq!(card.held_out.total, 2);
        assert_eq!(card.held_out.accuracy(), 0.5);
        assert_eq!(card.overall.total, 4);
    }

    #[test]
    fn difficulty_buckets_are_reported() {
        let card = build_scorecard(
            vec![
                case_result(Verdict::Exact, false, "easy"),
                case_result(Verdict::Wrong, false, "hard"),
                case_result(Verdict::Exact, false, "hard"),
            ],
            "m",
            "SQLite",
            0,
        );

        assert_eq!(card.by_difficulty["easy"].accuracy(), 1.0);
        assert_eq!(card.by_difficulty["hard"].accuracy(), 0.5);
    }

    #[test]
    fn bad_references_are_counted_and_excluded() {
        let card = build_scorecard(
            vec![
                case_result(Verdict::Exact, false, "easy"),
                case_result(Verdict::BadReference, false, "easy"),
            ],
            "m",
            "SQLite",
            0,
        );

        assert_eq!(card.bad_references, 1);
        assert_eq!(card.overall.total, 1);
        assert_eq!(card.overall.accuracy(), 1.0);
    }

    #[test]
    fn the_summary_leads_with_the_held_out_number() {
        let card = build_scorecard(
            vec![
                case_result(Verdict::Exact, false, "easy"),
                case_result(Verdict::Exact, true, "easy"),
            ],
            "m",
            "SQLite",
            0,
        );
        let summary = card.summary();
        assert!(summary.starts_with("held-out"), "{summary}");
        assert!(summary.contains("seen"));
    }

    #[test]
    fn a_scorecard_round_trips_through_json() {
        // CI diffs one run against another, so the format has to be stable.
        let card = build_scorecard(vec![case_result(Verdict::Exact, false, "easy")], "m", "SQLite", 5);
        let json = serde_json::to_string(&card).unwrap();
        let back: Scorecard = serde_json::from_str(&json).unwrap();
        assert_eq!(back.overall, card.overall);
        assert_eq!(back.cases.len(), 1);
    }

    #[test]
    fn regression_is_measured_on_held_out_accuracy() {
        let better = build_scorecard(
            vec![
                case_result(Verdict::Exact, false, "easy"),
                case_result(Verdict::Exact, false, "easy"),
            ],
            "m",
            "SQLite",
            0,
        );
        let worse = build_scorecard(
            vec![
                case_result(Verdict::Exact, false, "easy"),
                case_result(Verdict::Wrong, false, "easy"),
            ],
            "m",
            "SQLite",
            0,
        );

        assert!((better.regression_against(&worse) - 50.0).abs() < 1e-9);
        assert!((worse.regression_against(&better) + 50.0).abs() < 1e-9);
    }

    // ── Repeat statistics ────────────────────────────────────────────

    #[test]
    fn a_single_run_reports_itself_with_no_spread() {
        let r = Repeated::from_runs(vec![0.75]);
        assert_eq!(r.runs, 1);
        assert_eq!(r.mean, 0.75);
        assert_eq!(r.spread_points(), 0.0);
        assert!(
            r.summary().contains("too few"),
            "one run must not look authoritative: {}",
            r.summary()
        );
    }

    #[test]
    fn the_spread_is_reported_in_percentage_points() {
        // The real numbers this harness produced on an unchanged pipeline.
        let r = Repeated::from_runs(vec![0.75, 0.65, 0.60]);
        assert_eq!(r.runs, 3);
        assert!((r.mean - 0.6667).abs() < 0.001);
        assert!((r.spread_points() - 15.0).abs() < 0.001);
    }

    #[test]
    fn a_change_inside_the_spread_is_not_significant() {
        // This is the finding that motivated the whole thing: a 10-point
        // "improvement" measured against ±15-point noise means nothing.
        let baseline = Repeated::from_runs(vec![0.75, 0.65, 0.60]);
        assert!(!baseline.is_significant(10.0));
        assert!(!baseline.is_significant(-10.0));
        assert!(baseline.is_significant(20.0));
    }

    #[test]
    fn a_stable_pipeline_makes_small_changes_detectable() {
        let baseline = Repeated::from_runs(vec![0.80, 0.80, 0.81]);
        assert!(baseline.is_significant(5.0));
    }

    #[test]
    fn no_runs_is_zero_rather_than_a_division_by_zero() {
        let r = Repeated::from_runs(vec![]);
        assert_eq!(r.runs, 0);
        assert_eq!(r.mean, 0.0);
        assert_eq!(r.spread_points(), 0.0);
    }

    #[test]
    fn identical_runs_have_no_deviation() {
        // Floating point: the mean of three 0.7s is not bit-identical to 0.7,
        // so the deviation is ~1e-16 rather than exactly zero.
        let r = Repeated::from_runs(vec![0.7, 0.7, 0.7]);
        assert!(r.std_dev < 1e-9, "{}", r.std_dev);
        assert!(r.spread_points() < 1e-9);
    }

    #[test]
    fn the_summary_names_the_threshold_a_change_must_clear() {
        let r = Repeated::from_runs(vec![0.75, 0.60]);
        let summary = r.summary();
        assert!(summary.contains("15.0 points"), "{summary}");
        assert!(summary.contains("noise"), "{summary}");
    }

    // ── Case loading ─────────────────────────────────────────────────

    fn write_cases(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cases.json");
        std::fs::write(&path, body).unwrap();
        (dir, path)
    }

    #[test]
    fn cases_load_from_the_native_shape() {
        let (_d, path) = write_cases(
            r#"[{"question":"q","query":"SELECT 1;","difficulty":"easy","seen":true}]"#,
        );
        let cases = load_cases(&path).unwrap();
        assert_eq!(cases[0].question, "q");
        assert!(cases[0].seen);
    }

    #[test]
    fn spider_files_load_unmodified() {
        // Spider carries db_id and no difficulty or seen flag.
        let (_d, path) = write_cases(
            r#"[{"db_id":"concert_singer","question":"How many singers?","query":"SELECT count(*) FROM singer"}]"#,
        );
        let cases = load_cases(&path).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].difficulty, "unspecified");
        assert!(!cases[0].seen, "unseen is the conservative default");
    }

    #[test]
    fn sql_is_accepted_as_an_alias_for_query() {
        let (_d, path) = write_cases(r#"[{"question":"q","sql":"SELECT 1;"}]"#);
        assert_eq!(load_cases(&path).unwrap()[0].query, "SELECT 1;");
    }

    #[test]
    fn an_empty_case_file_is_an_error_not_a_perfect_score() {
        let (_d, path) = write_cases("[]");
        assert!(load_cases(&path).is_err());
    }

    #[test]
    fn a_malformed_case_file_reports_where_the_problem_is() {
        let (_d, path) = write_cases("{not json");
        let err = load_cases(&path).unwrap_err().to_string();
        assert!(err.contains("cases.json"), "{err}");
    }

    #[test]
    fn a_missing_file_is_an_error() {
        assert!(load_cases(Path::new("/nonexistent/cases.json")).is_err());
    }
}
