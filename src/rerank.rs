//! Schema-aware reranking of retrieved DDL.
//!
//! # The problem this targets
//!
//! Retrieval fuses BM25 and cosine over the *whole DDL string*. That treats a
//! table name and its fortieth column as equally important, and it penalises
//! wide tables: a `CREATE TABLE` with thirty columns dilutes every term in it,
//! so a narrow table that merely mentions a word can outrank the table the
//! question is actually about.
//!
//! Picking the wrong table is the dominant failure mode in text-to-SQL — once
//! the right DDL is missing from the prompt, no amount of prompting recovers
//! it. So this rescores candidates with structure the flat text does not carry:
//! a hit on a *table name* counts for more than a hit on a column name, and
//! both count for more than a hit anywhere else in the DDL.
//!
//! It reranks; it does not filter. The fused order is the tiebreak, so a
//! question that matches nothing structurally comes out exactly as retrieval
//! left it.

use sqlparser::ast::Statement;
use sqlparser::dialect::GenericDialect;
use sqlparser::parser::Parser;

use crate::retrieval::tokenize;

/// Weight for a question token matching a table name.
///
/// Higher than a column hit because naming the table is far stronger evidence:
/// "how many orders" is about `orders`, whichever table happens to have an
/// `order_id` column.
const TABLE_WEIGHT: f32 = 3.0;

/// Weight for a question token matching a column name.
const COLUMN_WEIGHT: f32 = 1.0;

/// Weight for a question token appearing anywhere else in the DDL — a comment,
/// a type name, a constraint. Weak, but not nothing.
const OTHER_WEIGHT: f32 = 0.25;

/// How much the original fused rank contributes, so retrieval's own judgement
/// is not thrown away.
const FUSION_WEIGHT: f32 = 1.5;

/// The structure we can extract from one DDL string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DdlShape {
    pub table: String,
    pub columns: Vec<String>,
}

/// Parse a `CREATE TABLE` into its table and column names, lowercased.
///
/// Returns `None` for anything that is not a parseable `CREATE TABLE` — the
/// categorical-hint entries stored alongside real DDL, for instance, which are
/// prose and should be scored on text alone.
pub fn shape_of(ddl: &str) -> Option<DdlShape> {
    let statements = Parser::parse_sql(&GenericDialect {}, ddl).ok()?;
    for statement in statements {
        if let Statement::CreateTable(create) = statement {
            let table = create
                .name
                .to_string()
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .trim_matches(|c| c == '"' || c == '`' || c == '[' || c == ']')
                .to_lowercase();
            return Some(DdlShape {
                table,
                columns: create
                    .columns
                    .iter()
                    .map(|c| c.name.value.to_lowercase())
                    .collect(),
            });
        }
    }
    None
}

/// Score one DDL against the question's tokens.
///
/// `fusion_score` is the normalised retrieval rank in `0.0..=1.0`, where 1.0 is
/// the top candidate.
pub fn score(question_tokens: &[String], ddl: &str, fusion_score: f32) -> f32 {
    let mut total = fusion_score * FUSION_WEIGHT;

    let Some(shape) = shape_of(ddl) else {
        // Not a CREATE TABLE — score it on plain text overlap so prose entries
        // (categorical hints, documentation) are not pushed to the bottom.
        let body = tokenize(ddl);
        let hits = question_tokens.iter().filter(|t| body.contains(t)).count();
        return total + hits as f32 * OTHER_WEIGHT;
    };

    // Table names are usually plural in the schema and singular in the
    // question ("orders" vs "order"), so match on both.
    let table_tokens = tokenize(&shape.table);
    let column_tokens: Vec<String> = shape.columns.iter().flat_map(|c| tokenize(c)).collect();

    for token in question_tokens {
        if table_tokens.iter().any(|t| matches_loosely(t, token)) {
            total += TABLE_WEIGHT;
        } else if column_tokens.iter().any(|c| matches_loosely(c, token)) {
            total += COLUMN_WEIGHT;
        }
    }
    total
}

/// Whether a schema token and a question token refer to the same thing.
///
/// Exact, or differing only by a trailing `s` — enough to bridge
/// "order"/"orders" and "category"/"categories" is not covered, which is fine:
/// this is a ranking hint, and a false negative only costs the boost.
fn matches_loosely(schema: &str, question: &str) -> bool {
    if schema == question {
        return true;
    }
    // Ignore very short tokens, where a plural rule produces noise ("a"/"as").
    if schema.len() < 3 || question.len() < 3 {
        return false;
    }
    schema.strip_suffix('s') == Some(question) || question.strip_suffix('s') == Some(schema)
}

/// Rerank retrieved DDL, returning at most `n`.
///
/// `candidates` arrive in retrieval order (best first). The result is reordered
/// but never filtered beyond the `n` cut, so a question with no structural
/// signal comes back in exactly the order it went in.
pub fn rerank(question: &str, candidates: Vec<String>, n: usize) -> Vec<String> {
    if candidates.len() <= 1 || n == 0 {
        return candidates.into_iter().take(n).collect();
    }

    let tokens = tokenize(question);
    let total = candidates.len() as f32;

    let mut scored: Vec<(f32, usize, String)> = candidates
        .into_iter()
        .enumerate()
        .map(|(i, ddl)| {
            // Linear decay: the top candidate scores 1.0, the last ~0.0.
            let fusion = 1.0 - (i as f32 / total);
            (score(&tokens, &ddl, fusion), i, ddl)
        })
        .collect();

    // Original position is the tiebreak, so equal scores keep retrieval's order.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });

    scored.into_iter().take(n).map(|(_, _, ddl)| ddl).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDERS: &str =
        "CREATE TABLE orders (id INTEGER, customer_id INTEGER, order_date TEXT, status TEXT);";
    const PRODUCTS: &str =
        "CREATE TABLE products (id INTEGER, name TEXT, category TEXT, price REAL);";
    const CUSTOMERS: &str =
        "CREATE TABLE customers (id INTEGER, name TEXT, country TEXT, city TEXT);";

    // ── Parsing ──────────────────────────────────────────────────────

    #[test]
    fn a_create_table_yields_its_table_and_columns() {
        let shape = shape_of(ORDERS).unwrap();
        assert_eq!(shape.table, "orders");
        assert_eq!(shape.columns, ["id", "customer_id", "order_date", "status"]);
    }

    #[test]
    fn a_schema_qualified_name_keeps_only_the_table() {
        let shape = shape_of("CREATE TABLE public.orders (id INT);").unwrap();
        assert_eq!(shape.table, "orders");
    }

    #[test]
    fn prose_is_not_a_table() {
        // Categorical hints live alongside DDL in the same store.
        assert!(shape_of("Column orders.status contains: pending, shipped.").is_none());
        assert!(shape_of("").is_none());
    }

    // ── Scoring ──────────────────────────────────────────────────────

    #[test]
    fn naming_the_table_outweighs_naming_a_column() {
        let tokens = tokenize("orders");
        let table_hit = score(&tokens, ORDERS, 0.0);

        // `customers` has no "orders" token anywhere.
        let no_hit = score(&tokens, CUSTOMERS, 0.0);
        assert!(table_hit > no_hit);

        // A table whose *column* matches scores lower than one whose name does.
        let column_hit = score(&tokenize("status"), ORDERS, 0.0);
        assert!(table_hit > column_hit, "{table_hit} vs {column_hit}");
    }

    #[test]
    fn a_singular_question_matches_a_plural_table() {
        // "how many orders" and "each order" should both find `orders`.
        let plural = score(&tokenize("orders"), ORDERS, 0.0);
        let singular = score(&tokenize("order"), ORDERS, 0.0);
        assert_eq!(plural, singular);
    }

    #[test]
    fn short_tokens_do_not_match_loosely() {
        // Otherwise "as" matches "a" and every table gets a spurious boost.
        assert!(!matches_loosely("a", "as"));
        assert!(!matches_loosely("is", "i"));
        assert!(matches_loosely("order", "orders"));
    }

    #[test]
    fn the_retrieval_rank_still_counts() {
        // A top-ranked candidate with no structural hit must not fall below a
        // bottom-ranked one that also has none.
        let tokens = tokenize("something unrelated");
        assert!(score(&tokens, ORDERS, 1.0) > score(&tokens, ORDERS, 0.0));
    }

    #[test]
    fn prose_entries_are_scored_on_text_rather_than_dropped() {
        let hint = "Column orders.status contains these values: pending, shipped.";
        let with_hit = score(&tokenize("shipped"), hint, 0.0);
        let without = score(&tokenize("zzzz"), hint, 0.0);
        assert!(with_hit > without);
    }

    // ── Reranking ────────────────────────────────────────────────────

    fn tables(ddls: &[String]) -> Vec<String> {
        ddls.iter()
            .map(|d| shape_of(d).map(|s| s.table).unwrap_or_else(|| "?".into()))
            .collect()
    }

    #[test]
    fn the_named_table_is_pulled_to_the_front() {
        // Retrieval put `orders` last; naming it in the question should fix that.
        let candidates = vec![CUSTOMERS.into(), PRODUCTS.into(), ORDERS.into()];
        let out = rerank("how many orders were cancelled?", candidates, 3);
        assert_eq!(tables(&out)[0], "orders", "{:?}", tables(&out));
    }

    #[test]
    fn a_question_with_no_structural_signal_preserves_retrieval_order() {
        // Reranking must not churn the order for nothing.
        let candidates = vec![CUSTOMERS.into(), PRODUCTS.into(), ORDERS.into()];
        let out = rerank("zzzz qqqq", candidates.clone(), 3);
        assert_eq!(out, candidates);
    }

    #[test]
    fn nothing_is_lost_when_n_covers_everything() {
        let candidates = vec![CUSTOMERS.into(), PRODUCTS.into(), ORDERS.into()];
        let out = rerank("orders", candidates.clone(), 3);
        assert_eq!(out.len(), 3);
        for ddl in &candidates {
            assert!(out.contains(ddl), "dropped {ddl}");
        }
    }

    #[test]
    fn the_cut_keeps_the_best_candidates() {
        let candidates = vec![CUSTOMERS.into(), PRODUCTS.into(), ORDERS.into()];
        let out = rerank("orders by status", candidates, 1);
        assert_eq!(tables(&out), vec!["orders"]);
    }

    #[test]
    fn multiple_named_tables_both_rise() {
        // A join question names two tables; both must reach the prompt.
        let candidates = vec![
            "CREATE TABLE unrelated_a (x INT);".to_string(),
            PRODUCTS.into(),
            "CREATE TABLE unrelated_b (y INT);".to_string(),
            ORDERS.into(),
        ];
        let out = rerank("revenue per product for each order", candidates, 2);
        let names = tables(&out);
        assert!(names.contains(&"products".to_string()), "{names:?}");
        assert!(names.contains(&"orders".to_string()), "{names:?}");
    }

    #[test]
    fn an_empty_candidate_list_is_handled() {
        assert!(rerank("anything", vec![], 5).is_empty());
    }

    #[test]
    fn a_single_candidate_is_returned_unchanged() {
        let one = vec![ORDERS.to_string()];
        assert_eq!(rerank("anything", one.clone(), 5), one);
    }

    #[test]
    fn asking_for_none_returns_none() {
        assert!(rerank("orders", vec![ORDERS.into()], 0).is_empty());
    }

    #[test]
    fn unparseable_candidates_do_not_break_the_rerank() {
        let candidates = vec![
            "this is not sql".to_string(),
            ORDERS.into(),
            "neither is this".to_string(),
        ];
        let out = rerank("orders", candidates, 3);
        assert_eq!(out.len(), 3);
        assert_eq!(tables(&out)[0], "orders");
    }
}
