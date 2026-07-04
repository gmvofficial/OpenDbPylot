//! The vector store: the RAG "memory".
//!
//! Stores three kinds of training material (DDL, documentation, question/SQL
//! pairs) and retrieves the most relevant items for a new question. It exposes
//! the vector-store methods: `add_ddl`, `add_documentation`,
//! `add_question_sql`, `get_related_ddl`, `get_related_documentation`,
//! `get_similar_question_sql`.

pub mod file;
pub mod memory;
#[cfg(feature = "qdrant")]
pub mod qdrant;

use anyhow::Result;
use async_trait::async_trait;

use crate::types::QuestionSql;

#[async_trait]
pub trait VectorStore: Send + Sync {
    async fn add_ddl(&self, ddl: &str) -> Result<()>;
    async fn add_documentation(&self, doc: &str) -> Result<()>;
    async fn add_question_sql(&self, question: &str, sql: &str) -> Result<()>;

    async fn get_related_ddl(&self, question: &str) -> Result<Vec<String>>;
    async fn get_related_documentation(&self, question: &str) -> Result<Vec<String>>;
    async fn get_similar_question_sql(&self, question: &str) -> Result<Vec<QuestionSql>>;

    /// Remove all stored DDL (table schema). Used by "Import schema" so a re-import
    /// replaces the schema instead of appending duplicates. Documentation and
    /// question/SQL examples are left untouched. Default is a no-op.
    async fn clear_ddl(&self) -> Result<()> {
        Ok(())
    }

    // --- Introspection (used by the CLI to show what's been trained). ---
    // Default to empty so existing stores don't have to implement them.
    async fn all_ddl(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn all_documentation(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn all_question_sql(&self) -> Result<Vec<QuestionSql>> {
        Ok(Vec::new())
    }
}
