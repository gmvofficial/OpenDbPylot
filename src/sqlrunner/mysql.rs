//! MySQL / MariaDB implementation of `SqlRunner` using sqlx with a lazy connection pool.

use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions, MySqlRow};
use sqlx::{Column, Row};
use tokio::sync::OnceCell;

use super::{QueryResult, SqlRunner};

/// Fail fast when first connecting instead of hanging the app on an unreachable host.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

pub struct MySqlRunner {
    url: String,
    pool: OnceCell<MySqlPool>,
}

impl MySqlRunner {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), pool: OnceCell::new() }
    }

    async fn pool(&self) -> Result<&MySqlPool> {
        self.pool
            .get_or_try_init(|| async {
                let connect = MySqlPoolOptions::new()
                    .max_connections(5)
                    .acquire_timeout(CONNECT_TIMEOUT)
                    .connect(&self.url);
                tokio::time::timeout(CONNECT_TIMEOUT, connect)
                    .await
                    .map_err(|_| anyhow::anyhow!("connection to MySQL timed out"))?
                    .context("failed to connect to MySQL")
            })
            .await
    }
}

fn mysql_cell_to_string(row: &MySqlRow, idx: usize) -> String {
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
    try_as!(u64);
    try_as!(u32);
    try_as!(f64);
    try_as!(f32);
    try_as!(bool);
    "NULL".to_string()
}

#[async_trait]
impl SqlRunner for MySqlRunner {
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
            out_rows.push((0..columns.len()).map(|i| mysql_cell_to_string(&row, i)).collect());
        }

        Ok(QueryResult { columns, rows: out_rows })
    }

    async fn introspect_schema(&self) -> Result<Vec<String>> {
        let pool = self.pool().await?;

        let rows = sqlx::query(
            "SELECT TABLE_NAME, COLUMN_NAME, DATA_TYPE, IS_NULLABLE \
             FROM information_schema.columns \
             WHERE TABLE_SCHEMA = DATABASE() \
             ORDER BY TABLE_NAME, ORDINAL_POSITION",
        )
        .fetch_all(pool)
        .await
        .context("failed to query information_schema")?;

        let mut table_cols: Vec<(String, Vec<String>)> = Vec::new();
        for row in &rows {
            let table: String = row.try_get("TABLE_NAME").unwrap_or_default();
            let col: String = row.try_get("COLUMN_NAME").unwrap_or_default();
            let dtype: String = row.try_get("DATA_TYPE").unwrap_or_default();
            let nullable: String = row.try_get("IS_NULLABLE").unwrap_or_default();
            let col_def = if nullable == "NO" {
                format!("  `{}` {} NOT NULL", col, dtype)
            } else {
                format!("  `{}` {}", col, dtype)
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
            .map(|(t, cols)| format!("CREATE TABLE `{}` (\n{}\n);", t, cols.join(",\n")))
            .collect())
    }
}
