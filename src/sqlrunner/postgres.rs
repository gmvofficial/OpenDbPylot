//! PostgreSQL implementation of `SqlRunner` using sqlx with a lazy connection pool.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::postgres::{PgPool, PgPoolOptions, PgRow};
use sqlx::{Column, Row};
use tokio::sync::OnceCell;

use super::{QueryResult, SqlRunner};

/// How long to wait when first connecting before giving up (so an unreachable
/// host fails fast instead of hanging the app).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PostgresRunner {
    url: String,
    pool: OnceCell<PgPool>,
}

impl PostgresRunner {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), pool: OnceCell::new() }
    }

    async fn pool(&self) -> Result<&PgPool> {
        self.pool
            .get_or_try_init(|| async {
                let connect = PgPoolOptions::new()
                    .max_connections(5)
                    .acquire_timeout(CONNECT_TIMEOUT)
                    .connect(&self.url);
                tokio::time::timeout(CONNECT_TIMEOUT, connect)
                    .await
                    .map_err(|_| anyhow::anyhow!("connection to PostgreSQL timed out"))?
                    .context("failed to connect to PostgreSQL")
            })
            .await
    }
}

/// Convert a postgres row cell to String, trying common types in order.
fn pg_cell_to_string(row: &PgRow, idx: usize) -> String {
    macro_rules! try_as {
        ($T:ty) => {
            if let Ok(v) = row.try_get::<Option<$T>, _>(idx) {
                return v.map(|x| x.to_string()).unwrap_or_else(|| "NULL".to_string());
            }
        };
    }
    try_as!(String);
    try_as!(i64);
    try_as!(i32);
    try_as!(i16);
    try_as!(f64);
    try_as!(f32);
    try_as!(bool);
    "NULL".to_string()
}

#[async_trait]
impl SqlRunner for PostgresRunner {
    async fn run_sql(&self, sql: &str) -> Result<QueryResult> {
        use tokio_stream::StreamExt;

        let pool = self.pool().await?;
        // Stream rows and stop at MAX_ROWS so a huge result can't exhaust memory.
        let stream = sqlx::query(sql).fetch(pool);
        tokio::pin!(stream);

        let mut columns: Vec<String> = Vec::new();
        let mut out_rows: Vec<Vec<String>> = Vec::new();
        while let Some(item) = stream.next().await {
            let row = item.context("query failed")?;
            if columns.is_empty() {
                columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            }
            if out_rows.len() >= super::MAX_ROWS {
                break;
            }
            out_rows.push((0..columns.len()).map(|i| pg_cell_to_string(&row, i)).collect());
        }

        Ok(QueryResult { columns, rows: out_rows })
    }

    async fn introspect_schema(&self) -> Result<Vec<String>> {
        let pool = self.pool().await?;

        // Fetch all columns for user tables in the public schema, ordered by table + position.
        let rows = sqlx::query(
            "SELECT table_name, column_name, data_type, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'public' \
             ORDER BY table_name, ordinal_position",
        )
        .fetch_all(pool)
        .await
        .context("failed to query information_schema")?;

        // Group columns by table and build CREATE TABLE DDL strings.
        let mut table_cols: Vec<(String, Vec<String>)> = Vec::new();
        for row in &rows {
            let table: String = row.try_get("table_name").unwrap_or_default();
            let col: String = row.try_get("column_name").unwrap_or_default();
            let dtype: String = row.try_get("data_type").unwrap_or_default();
            let nullable: String = row.try_get("is_nullable").unwrap_or_default();
            let col_def = if nullable == "NO" {
                format!("  {} {} NOT NULL", col, dtype)
            } else {
                format!("  {} {}", col, dtype)
            };
            if let Some(last) = table_cols.last_mut() {
                if last.0 == table {
                    last.1.push(col_def);
                    continue;
                }
            }
            table_cols.push((table, vec![col_def]));
        }

        Ok(table_cols
            .into_iter()
            .map(|(t, cols)| format!("CREATE TABLE {} (\n{}\n);", t, cols.join(",\n")))
            .collect())
    }
}
