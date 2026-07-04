//! `ContextEnhancer` — injects per-question context into the system prompt
//! (opendbpylot 2.0's `LlmContextEnhancer`).
//!
//! The agent calls `enhance(question)` each turn and appends the result to its
//! base system prompt. `RagEnhancer` is the SQL grounding: it retrieves the most
//! relevant DDL, documentation, and example question/SQL pairs from the vector
//! store — the same retrieval the legacy `OpenDbPylot::generate_sql_inner` did, now
//! reusable by the agent loop.

use std::sync::Arc;

use async_trait::async_trait;

use crate::vectorstore::VectorStore;

#[async_trait]
pub trait ContextEnhancer: Send + Sync {
    /// Extra context to append to the system prompt for this question.
    /// Return an empty string to add nothing.
    async fn enhance(&self, question: &str) -> String;
}

/// Retrieves relevant training material (DDL / docs / examples) for the question.
pub struct RagEnhancer {
    store: Arc<dyn VectorStore>,
}

impl RagEnhancer {
    pub fn new(store: Arc<dyn VectorStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl ContextEnhancer for RagEnhancer {
    async fn enhance(&self, question: &str) -> String {
        let ddl = self.store.get_related_ddl(question).await.unwrap_or_default();
        let docs = self.store.get_related_documentation(question).await.unwrap_or_default();
        let examples = self.store.get_similar_question_sql(question).await.unwrap_or_default();

        let mut out = String::new();

        if !ddl.is_empty() {
            out.push_str("\n===Tables\n");
            for d in &ddl {
                out.push_str(d);
                out.push_str("\n\n");
            }
        }

        if !docs.is_empty() {
            out.push_str("\n===Additional Context\n\n");
            for d in &docs {
                out.push_str(d);
                out.push_str("\n\n");
            }
        }

        if !examples.is_empty() {
            out.push_str("\n===Example question/SQL pairs (for reference)\n\n");
            for ex in &examples {
                out.push_str(&format!("Q: {}\nSQL: {}\n\n", ex.question, ex.sql));
            }
        }

        out
    }
}
