//! The training review queue.
//!
//! # Why
//!
//! `auto_train` stores every question whose SQL returned rows as a new
//! few-shot example. "Returned rows" is not "was correct": a query that
//! silently answers a slightly different question still returns rows, becomes
//! an exemplar, and then teaches the same mistake to every later question that
//! retrieves it. The corpus is the quality ceiling of a RAG pipeline, so an
//! unreviewed corpus lowers that ceiling over time.
//!
//! Because of that the flag defaults to off — which means the pipeline never
//! learns from use at all. Both states are bad.
//!
//! A queue fixes both. Captures land here instead of in retrieval; nothing
//! reaches the corpus until a human approves it. That makes `auto_train` safe
//! to turn on, and gives the corpus a curation surface it did not have.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::types::QuestionSql;

/// Filename inside the app's home directory.
pub const DEFAULT_FILENAME: &str = "pending-training.json";

/// Cap on queued items. A busy install should not accumulate thousands of
/// unreviewed pairs; past this the oldest are dropped, since a stale capture is
/// the least useful thing in the queue.
pub const MAX_PENDING: usize = 500;

/// A question/SQL pair awaiting review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingExample {
    /// Stable id, used to approve or reject this specific item.
    pub id: String,
    pub question: String,
    pub sql: String,
    /// Unix seconds. Plain integer so this file needs no date library.
    pub captured_at: i64,
    /// How many rows the query returned when it was captured. Useful context
    /// for a reviewer: zero rows is a common sign of a subtly wrong query.
    pub row_count: usize,
}

impl PendingExample {
    /// Content-addressed id, so capturing the same pair twice does not queue
    /// two copies for a reviewer to dismiss separately.
    pub fn id_for(question: &str, sql: &str) -> String {
        let normalized = format!("{}|{}", normalize(question), normalize(sql));
        // FNV-1a: short, stable across runs, and needs no dependency. Collision
        // resistance does not matter here — a collision costs one duplicate.
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in normalized.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        format!("{hash:016x}")
    }

    pub fn new(question: &str, sql: &str, row_count: usize) -> Self {
        Self {
            id: Self::id_for(question, sql),
            question: question.trim().to_string(),
            sql: sql.trim().to_string(),
            captured_at: now_unix(),
            row_count,
        }
    }

    pub fn as_pair(&self) -> QuestionSql {
        QuestionSql {
            question: self.question.clone(),
            sql: self.sql.clone(),
        }
    }
}

/// Collapse whitespace and case so trivially different spellings of the same
/// pair produce the same id.
fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// What happened to a capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Captured {
    /// Newly queued.
    Queued,
    /// Already waiting for review.
    AlreadyPending,
    /// Already approved once; not re-queued.
    AlreadyApproved,
    /// Rejected before; not re-queued.
    PreviouslyRejected,
}

/// The on-disk queue.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewQueue {
    pub pending: Vec<PendingExample>,
    /// Ids already approved, so re-answering the same question does not put it
    /// back in front of the reviewer.
    #[serde(default)]
    pub approved: HashSet<String>,
    /// Ids explicitly rejected. A rejected pair must stay rejected — otherwise
    /// the reviewer dismisses the same bad example every time it recurs.
    #[serde(default)]
    pub rejected: HashSet<String>,
}

impl ReviewQueue {
    /// Path of the queue file for a given home directory.
    pub fn path_in(home: &Path) -> PathBuf {
        home.join(DEFAULT_FILENAME)
    }

    /// Load, treating a missing file as an empty queue.
    ///
    /// A corrupt file is an error rather than a silent reset: quietly throwing
    /// away someone's review backlog is worse than refusing to start.
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("could not read {}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(&text)
            .with_context(|| format!("could not parse the review queue at {}", path.display()))
    }

    /// Write the queue, replacing the file atomically.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json)
            .with_context(|| format!("could not write {}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("could not replace {}", path.display()))?;
        Ok(())
    }

    /// Queue a capture, unless it has been seen before.
    pub fn capture(&mut self, question: &str, sql: &str, row_count: usize) -> Captured {
        let example = PendingExample::new(question, sql, row_count);

        if self.approved.contains(&example.id) {
            return Captured::AlreadyApproved;
        }
        if self.rejected.contains(&example.id) {
            return Captured::PreviouslyRejected;
        }
        if self.pending.iter().any(|p| p.id == example.id) {
            return Captured::AlreadyPending;
        }

        self.pending.push(example);
        if self.pending.len() > MAX_PENDING {
            let overflow = self.pending.len() - MAX_PENDING;
            self.pending.drain(0..overflow);
        }
        Captured::Queued
    }

    /// Approve an item, returning the pair to add to the training corpus.
    ///
    /// `None` means no such pending id — approving twice is a no-op rather
    /// than a duplicate training example.
    pub fn approve(&mut self, id: &str) -> Option<QuestionSql> {
        let index = self.pending.iter().position(|p| p.id == id)?;
        let example = self.pending.remove(index);
        self.approved.insert(example.id.clone());
        Some(example.as_pair())
    }

    /// Approve everything waiting, returning the pairs in queue order.
    pub fn approve_all(&mut self) -> Vec<QuestionSql> {
        let drained: Vec<PendingExample> = self.pending.drain(..).collect();
        drained
            .into_iter()
            .map(|example| {
                self.approved.insert(example.id.clone());
                example.as_pair()
            })
            .collect()
    }

    /// Reject an item so it never returns. Reports whether it was found.
    pub fn reject(&mut self, id: &str) -> bool {
        let Some(index) = self.pending.iter().position(|p| p.id == id) else {
            return false;
        };
        let example = self.pending.remove(index);
        self.rejected.insert(example.id);
        true
    }

    /// Reject everything waiting, returning how many.
    pub fn reject_all(&mut self) -> usize {
        let count = self.pending.len();
        for example in self.pending.drain(..) {
            self.rejected.insert(example.id);
        }
        count
    }

    /// Find one pending item by id, or by a unique prefix of it — so a reviewer
    /// can type the first few characters rather than all sixteen.
    pub fn resolve_id(&self, prefix: &str) -> Option<&PendingExample> {
        let prefix = prefix.trim();
        if prefix.is_empty() {
            return None;
        }
        let mut matches = self.pending.iter().filter(|p| p.id.starts_with(prefix));
        let first = matches.next()?;
        // Ambiguous prefixes must not silently pick one.
        matches.next().is_none().then_some(first)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue() -> ReviewQueue {
        ReviewQueue::default()
    }

    // ── Identity ─────────────────────────────────────────────────────

    #[test]
    fn the_same_pair_gets_the_same_id() {
        assert_eq!(
            PendingExample::id_for("how many?", "SELECT COUNT(*) FROM t;"),
            PendingExample::id_for("how many?", "SELECT COUNT(*) FROM t;")
        );
    }

    #[test]
    fn whitespace_and_case_do_not_change_the_id() {
        // Otherwise the same pair queues twice for a reviewer to dismiss twice.
        assert_eq!(
            PendingExample::id_for("How Many?", "SELECT  COUNT(*)\nFROM t;"),
            PendingExample::id_for("how many?", "SELECT COUNT(*) FROM t;")
        );
    }

    #[test]
    fn different_pairs_get_different_ids() {
        assert_ne!(
            PendingExample::id_for("a", "SELECT 1;"),
            PendingExample::id_for("a", "SELECT 2;")
        );
        assert_ne!(
            PendingExample::id_for("a", "SELECT 1;"),
            PendingExample::id_for("b", "SELECT 1;")
        );
    }

    // ── Capture ──────────────────────────────────────────────────────

    #[test]
    fn a_capture_is_queued_not_trained() {
        // The whole point: nothing reaches retrieval without a human.
        let mut q = queue();
        assert_eq!(q.capture("how many?", "SELECT COUNT(*) FROM t;", 1), Captured::Queued);
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn capturing_the_same_pair_twice_queues_it_once() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        assert_eq!(q.capture("q", "SELECT 1;", 1), Captured::AlreadyPending);
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn an_approved_pair_is_not_queued_again() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        let id = q.pending[0].id.clone();
        q.approve(&id);

        assert_eq!(q.capture("q", "SELECT 1;", 1), Captured::AlreadyApproved);
        assert!(q.is_empty(), "already in the corpus — nothing to review");
    }

    #[test]
    fn a_rejected_pair_never_comes_back() {
        // Otherwise the reviewer dismisses the same bad example forever.
        let mut q = queue();
        q.capture("q", "SELECT wrong;", 0);
        let id = q.pending[0].id.clone();
        q.reject(&id);

        assert_eq!(q.capture("q", "SELECT wrong;", 0), Captured::PreviouslyRejected);
        assert!(q.is_empty());
    }

    #[test]
    fn the_queue_is_capped() {
        let mut q = queue();
        for i in 0..(MAX_PENDING + 20) {
            q.capture(&format!("question {i}"), "SELECT 1;", 1);
        }
        assert_eq!(q.pending_count(), MAX_PENDING);
        assert_eq!(
            q.pending.last().unwrap().question,
            format!("question {}", MAX_PENDING + 19),
            "the newest capture must survive the trim"
        );
    }

    #[test]
    fn the_row_count_is_kept_for_the_reviewer() {
        // Zero rows is a common sign of a subtly wrong query.
        let mut q = queue();
        q.capture("q", "SELECT 1 WHERE 0;", 0);
        assert_eq!(q.pending[0].row_count, 0);
    }

    // ── Approve and reject ───────────────────────────────────────────

    #[test]
    fn approving_returns_the_pair_to_train_on() {
        let mut q = queue();
        q.capture("how many?", "SELECT COUNT(*) FROM t;", 1);
        let id = q.pending[0].id.clone();

        let pair = q.approve(&id).expect("the pending item should be found");
        assert_eq!(pair.question, "how many?");
        assert_eq!(pair.sql, "SELECT COUNT(*) FROM t;");
        assert!(q.is_empty());
    }

    #[test]
    fn approving_twice_does_not_produce_a_second_training_example() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        let id = q.pending[0].id.clone();

        assert!(q.approve(&id).is_some());
        assert!(q.approve(&id).is_none(), "the second approve is a no-op");
    }

    #[test]
    fn approving_an_unknown_id_is_a_no_op() {
        let mut q = queue();
        assert!(q.approve("nope").is_none());
    }

    #[test]
    fn rejecting_reports_whether_it_found_anything() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        let id = q.pending[0].id.clone();

        assert!(q.reject(&id));
        assert!(!q.reject(&id), "already gone");
    }

    #[test]
    fn approve_all_returns_every_pair_in_order() {
        let mut q = queue();
        q.capture("first", "SELECT 1;", 1);
        q.capture("second", "SELECT 2;", 1);

        let pairs = q.approve_all();
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].question, "first");
        assert_eq!(pairs[1].question, "second");
        assert!(q.is_empty());
    }

    #[test]
    fn reject_all_reports_the_count_and_remembers_them() {
        let mut q = queue();
        q.capture("a", "SELECT 1;", 1);
        q.capture("b", "SELECT 2;", 1);

        assert_eq!(q.reject_all(), 2);
        assert!(q.is_empty());
        assert_eq!(q.capture("a", "SELECT 1;", 1), Captured::PreviouslyRejected);
    }

    // ── Id resolution ────────────────────────────────────────────────

    #[test]
    fn a_unique_prefix_resolves() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        let id = q.pending[0].id.clone();

        assert_eq!(q.resolve_id(&id[..6]).unwrap().id, id);
        assert_eq!(q.resolve_id(&id).unwrap().id, id);
    }

    #[test]
    fn an_ambiguous_prefix_resolves_to_nothing_rather_than_guessing() {
        // Approving the wrong example because a prefix was short is exactly the
        // mistake this queue exists to prevent.
        let mut q = queue();
        q.pending.push(PendingExample {
            id: "abc111".into(),
            question: "one".into(),
            sql: "SELECT 1;".into(),
            captured_at: 0,
            row_count: 1,
        });
        q.pending.push(PendingExample {
            id: "abc222".into(),
            question: "two".into(),
            sql: "SELECT 2;".into(),
            captured_at: 0,
            row_count: 1,
        });

        assert!(q.resolve_id("abc").is_none());
        assert_eq!(q.resolve_id("abc1").unwrap().question, "one");
    }

    #[test]
    fn an_empty_prefix_resolves_to_nothing() {
        let mut q = queue();
        q.capture("q", "SELECT 1;", 1);
        assert!(q.resolve_id("").is_none());
        assert!(q.resolve_id("   ").is_none());
    }

    // ── Persistence ──────────────────────────────────────────────────

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = ReviewQueue::path_in(dir.path());
        (dir, path)
    }

    #[test]
    fn the_queue_round_trips_through_disk() {
        let (_d, path) = temp();
        let mut q = queue();
        q.capture("how many?", "SELECT COUNT(*) FROM t;", 3);
        q.save(&path).unwrap();

        let loaded = ReviewQueue::load(&path).unwrap();
        assert_eq!(loaded.pending_count(), 1);
        assert_eq!(loaded.pending[0].question, "how many?");
        assert_eq!(loaded.pending[0].row_count, 3);
    }

    #[test]
    fn approvals_and_rejections_survive_a_reload() {
        // Without this, every restart re-queues everything already decided.
        let (_d, path) = temp();
        let mut q = queue();
        q.capture("good", "SELECT 1;", 1);
        q.capture("bad", "SELECT 2;", 0);
        let good = q.pending[0].id.clone();
        let bad = q.pending[1].id.clone();
        q.approve(&good);
        q.reject(&bad);
        q.save(&path).unwrap();

        let mut loaded = ReviewQueue::load(&path).unwrap();
        assert_eq!(loaded.capture("good", "SELECT 1;", 1), Captured::AlreadyApproved);
        assert_eq!(loaded.capture("bad", "SELECT 2;", 0), Captured::PreviouslyRejected);
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_queue() {
        let (_d, path) = temp();
        assert!(ReviewQueue::load(&path).unwrap().is_empty());
    }

    #[test]
    fn an_empty_file_loads_as_an_empty_queue() {
        let (_d, path) = temp();
        std::fs::write(&path, "  \n").unwrap();
        assert!(ReviewQueue::load(&path).unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_silent_reset() {
        // Quietly discarding a review backlog is worse than refusing to load.
        let (_d, path) = temp();
        std::fs::write(&path, "{not json").unwrap();
        let err = ReviewQueue::load(&path).unwrap_err().to_string();
        assert!(err.contains("review queue"), "{err}");
    }

    #[test]
    fn a_queue_written_by_an_older_version_still_loads() {
        // `approved` and `rejected` were added after `pending`; a file without
        // them must not fail to parse.
        let (_d, path) = temp();
        std::fs::write(
            &path,
            r#"{"pending":[{"id":"x","question":"q","sql":"SELECT 1;","captured_at":0,"row_count":1}]}"#,
        )
        .unwrap();

        let loaded = ReviewQueue::load(&path).unwrap();
        assert_eq!(loaded.pending_count(), 1);
        assert!(loaded.approved.is_empty());
    }
}
