//! SQLite implementation of the `SqlRunner` trait.
//!
//! Concrete plug-in, like `src/opendbpylot/integrations/sqlite/` in the Python project.
//!
//! `rusqlite` is synchronous, so all database work runs inside `spawn_blocking`
//! — otherwise a big/slow query would block a Tokio worker thread and stall the
//! whole server.

use anyhow::{Context, Result};
use async_trait::async_trait;
use rusqlite::Connection;

use super::{QueryResult, SqlRunner, MAX_ROWS};

/// Runs SQL against a SQLite database file on disk.
pub struct SqliteRunner {
    path: String,
}

impl SqliteRunner {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into() }
    }

    /// Synchronous query — always called inside `spawn_blocking`.
    fn run_sql_blocking(path: &str, sql: &str) -> Result<QueryResult> {
        let conn = Connection::open(path).context("failed to open SQLite database")?;
        let mut stmt = conn.prepare(sql).context("failed to prepare SQL")?;

        let column_count = stmt.column_count();
        let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

        let mut out_rows: Vec<Vec<String>> = Vec::new();
        let mut rows = stmt.query([]).context("failed to execute query")?;
        while let Some(row) = rows.next().context("failed to read row")? {
            if out_rows.len() >= MAX_ROWS {
                break; // cap memory on huge result sets
            }
            let mut record = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let value: rusqlite::types::Value = row.get(i)?;
                let cell = match value {
                    rusqlite::types::Value::Null => "NULL".to_string(),
                    rusqlite::types::Value::Integer(n) => n.to_string(),
                    rusqlite::types::Value::Real(f) => f.to_string(),
                    rusqlite::types::Value::Text(t) => t,
                    rusqlite::types::Value::Blob(_) => "<blob>".to_string(),
                };
                record.push(cell);
            }
            out_rows.push(record);
        }

        Ok(QueryResult { columns, rows: out_rows })
    }

    fn introspect_blocking(path: &str) -> Result<Vec<String>> {
        let conn = Connection::open(path).context("failed to open SQLite database")?;
        let mut stmt = conn.prepare(
            "SELECT name, sql FROM sqlite_master WHERE type='table' AND sql IS NOT NULL ORDER BY name",
        )?;
        let mut ddls = Vec::new();
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let ddl: String = row.get(1)?;
            ddls.push(ddl);
        }
        Ok(ddls)
    }

    /// List distinct values for text columns that have few of them, so the model
    /// knows the real vocabulary (categories, statuses, countries, product names…).
    fn hints_blocking(path: &str) -> Result<Vec<String>> {
        const MAX_DISTINCT: usize = 50;

        let conn = Connection::open(path).context("failed to open SQLite database")?;

        // User tables.
        let tables: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
            )?;
            let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
            rows.filter_map(|r| r.ok()).collect()
        };

        let mut hints = Vec::new();
        for table in &tables {
            // Columns of this table: (name, type).
            let cols: Vec<(String, String)> = {
                let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
                rows.filter_map(|r| r.ok()).collect()
            };

            for (col, ctype) in cols {
                let t = ctype.to_uppercase();
                let is_text = t.is_empty() || t.contains("CHAR") || t.contains("TEXT") || t.contains("CLOB");
                if !is_text {
                    continue;
                }

                // Count distinct, but stop at MAX_DISTINCT+1 so it's cheap on huge tables.
                let capped: i64 = conn
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM (SELECT DISTINCT \"{col}\" FROM \"{table}\" \
                             WHERE \"{col}\" IS NOT NULL LIMIT {})",
                            MAX_DISTINCT + 1
                        ),
                        [],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);

                if capped < 2 || capped as usize > MAX_DISTINCT {
                    continue; // skip constant or high-cardinality columns
                }

                let values: Vec<String> = {
                    let mut stmt = conn.prepare(&format!(
                        "SELECT DISTINCT \"{col}\" FROM \"{table}\" WHERE \"{col}\" IS NOT NULL LIMIT {MAX_DISTINCT}"
                    ))?;
                    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
                    rows.filter_map(|r| r.ok()).collect()
                };

                if !values.is_empty() {
                    hints.push(format!(
                        "Column {table}.{col} has these values: {}.",
                        values.join(", ")
                    ));
                }
            }
        }
        Ok(hints)
    }
}

#[async_trait]
impl SqlRunner for SqliteRunner {
    async fn introspect_schema(&self) -> Result<Vec<String>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || Self::introspect_blocking(&path))
            .await
            .context("sqlite task failed")?
    }

    async fn categorical_hints(&self) -> Result<Vec<String>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || Self::hints_blocking(&path))
            .await
            .context("sqlite task failed")?
    }

    async fn run_sql(&self, sql: &str) -> Result<QueryResult> {
        let path = self.path.clone();
        let sql = sql.to_string();
        tokio::task::spawn_blocking(move || Self::run_sql_blocking(&path, &sql))
            .await
            .context("sqlite task failed")?
    }
}
