//! The SQL execution layer: anything that can run SQL and return rows.
//!
//! implementation of `run_sql` + the `SqlRunner` capability.
//! The trait hides the specific database, so a Postgres or MySQL runner
//! can drop in later without changing the rest of the app.

pub mod sqlite;
#[cfg(feature = "remote-db")]
pub mod postgres;
#[cfg(feature = "remote-db")]
pub mod mysql;
#[cfg(feature = "duckdb")]
pub mod duckdb;

use anyhow::Result;
use async_trait::async_trait;

/// Hard cap on rows pulled into memory for a single query, to avoid OOM on huge
/// result sets. Runners stop collecting past this many rows.
pub const MAX_ROWS: usize = 10_000;

/// The result of a query: column names plus rows of stringified values.
///
/// We keep everything as `String` for now so it's easy to print. A real
/// system would keep proper types (this is where `polars` would come in).
#[derive(Debug, serde::Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl QueryResult {
    /// Render the result as a simple pipe-separated text table.
    /// Used when feeding intermediate query results back to the LLM.
    pub fn to_text(&self) -> String {
        if self.columns.is_empty() {
            return "(no rows)".to_string();
        }
        let mut out = self.columns.join(" | ");
        out.push('\n');
        for row in &self.rows {
            out.push_str(&row.join(" | "));
            out.push('\n');
        }
        out
    }

    /// Print the result as a simple text table.
    pub fn print_table(&self) {
        if self.columns.is_empty() {
            println!("(statement ran; no rows returned)");
            return;
        }
        let header = self.columns.join(" | ");
        println!("{header}");
        println!("{}", "-".repeat(header.len()));
        for row in &self.rows {
            println!("{}", row.join(" | "));
        }
        println!("\n({} row(s))", self.rows.len());
    }
}

/// The contract every database runner must fulfill.
#[async_trait]
pub trait SqlRunner: Send + Sync {
    /// Execute a SQL statement and return its results.
    async fn run_sql(&self, sql: &str) -> Result<QueryResult>;

    /// Return DDL strings (one per table) describing the database schema.
    /// Used by the "Import schema" training action.
    async fn introspect_schema(&self) -> Result<Vec<String>> {
        Err(anyhow::anyhow!("schema introspection not supported for this runner"))
    }

    /// Return human-readable hints listing the distinct values of low-cardinality
    /// text columns (e.g. "products.category has values: Electronics, Books, …").
    /// This lets the model filter on values the user names without guessing the
    /// exact spelling. Default: none.
    async fn categorical_hints(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}
