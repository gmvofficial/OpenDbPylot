//! DuckDB implementation of the `SqlRunner` trait (enable with `--features duckdb`).
//!
//! DuckDB is the killer backend for a *personal* data agent: besides regular
//! tables it can query local files directly —
//! `SELECT * FROM 'sales.csv'`, `SELECT * FROM 'data.parquet'` — so users can
//! point opendbpylot at a folder of exports with no ETL.
//!
//! The `duckdb` crate is synchronous (a rusqlite fork) and a `Connection` is
//! `Send` but not `Sync`, so we hold a single connection behind a `Mutex` and do
//! all work inside `spawn_blocking`. A single shared connection (rather than
//! open-per-query) is also required for `:memory:` to persist between calls —
//! each fresh in-memory connection would otherwise be an empty database.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use duckdb::types::ValueRef;
use duckdb::Connection;

use super::{QueryResult, SqlRunner, MAX_ROWS};

/// Runs SQL against a DuckDB database file (or `:memory:`).
pub struct DuckDbRunner {
    conn: Arc<Mutex<Connection>>,
}

impl DuckDbRunner {
    /// Open (or create) the database. `path` may be a `.duckdb` file or
    /// `":memory:"` (also the empty string). Fallible because a bad path/file
    /// should fail loudly at connect time, not on the first query.
    pub fn new(path: impl Into<String>) -> Result<Self> {
        let path = path.into();
        let conn = if path.is_empty() || path == ":memory:" {
            Connection::open_in_memory().context("failed to open in-memory DuckDB")?
        } else {
            Connection::open(&path).context("failed to open DuckDB database")?
        };
        Ok(Self { conn: Arc::new(Mutex::new(conn)) })
    }

    fn run_sql_blocking(conn: &Connection, sql: &str) -> Result<QueryResult> {
        let mut stmt = conn.prepare(sql).context("failed to prepare SQL")?;
        let mut rows = stmt.query([]).context("failed to execute query")?;

        // Column names are only known after the first `query()`.
        let columns: Vec<String> = rows
            .as_ref()
            .map(|s| s.column_names().iter().map(|c| c.to_string()).collect())
            .unwrap_or_default();
        let column_count = columns.len();

        let mut out_rows: Vec<Vec<String>> = Vec::new();
        while let Some(row) = rows.next().context("failed to read row")? {
            if out_rows.len() >= MAX_ROWS {
                break; // cap memory on huge result sets
            }
            let mut record = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let vr = row.get_ref(i).context("failed to read cell")?;
                record.push(value_to_string(vr));
            }
            out_rows.push(record);
        }

        Ok(QueryResult { columns, rows: out_rows })
    }

    fn introspect_blocking(conn: &Connection) -> Result<Vec<String>> {
        let mut stmt = conn.prepare(
            "SELECT table_name, column_name, data_type, is_nullable \
             FROM information_schema.columns \
             WHERE table_schema = 'main' \
             ORDER BY table_name, ordinal_position",
        )?;

        let mut rows = stmt.query([])?;
        let mut table_cols: Vec<(String, Vec<String>)> = Vec::new();
        while let Some(row) = rows.next()? {
            let table: String = row.get(0)?;
            let col: String = row.get(1)?;
            let dtype: String = row.get(2)?;
            let nullable: String = row.get(3)?;
            let col_def = if nullable == "NO" {
                format!("  {col} {dtype} NOT NULL")
            } else {
                format!("  {col} {dtype}")
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
            .map(|(t, cols)| format!("CREATE TABLE {t} (\n{}\n);", cols.join(",\n")))
            .collect())
    }
}

/// Stringify a DuckDB cell. DuckDB has a rich type system (decimals, timestamps,
/// lists, structs…); we cover the scalar types explicitly and fall back to the
/// value's debug form for the exotic ones, so no column type can panic.
fn value_to_string(vr: ValueRef<'_>) -> String {
    match vr {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Boolean(b) => b.to_string(),
        ValueRef::TinyInt(n) => n.to_string(),
        ValueRef::SmallInt(n) => n.to_string(),
        ValueRef::Int(n) => n.to_string(),
        ValueRef::BigInt(n) => n.to_string(),
        ValueRef::HugeInt(n) => n.to_string(),
        ValueRef::UTinyInt(n) => n.to_string(),
        ValueRef::USmallInt(n) => n.to_string(),
        ValueRef::UInt(n) => n.to_string(),
        ValueRef::UBigInt(n) => n.to_string(),
        ValueRef::Float(f) => f.to_string(),
        ValueRef::Double(f) => f.to_string(),
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        ValueRef::Blob(_) => "<blob>".to_string(),
        other => format!("{other:?}"),
    }
}

#[async_trait]
impl SqlRunner for DuckDbRunner {
    async fn run_sql(&self, sql: &str) -> Result<QueryResult> {
        let conn = self.conn.clone();
        let sql = sql.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            Self::run_sql_blocking(&conn, &sql)
        })
        .await
        .context("duckdb task failed")?
    }

    async fn introspect_schema(&self) -> Result<Vec<String>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock().unwrap();
            Self::introspect_blocking(&conn)
        })
        .await
        .context("duckdb task failed")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn runs_query_and_introspects() {
        let db = DuckDbRunner::new(":memory:").unwrap();
        db.run_sql("CREATE TABLE t (id INTEGER, label VARCHAR)").await.unwrap();
        db.run_sql("INSERT INTO t VALUES (1, 'a'), (2, 'b')").await.unwrap();

        let res = db.run_sql("SELECT id, label FROM t ORDER BY id").await.unwrap();
        assert_eq!(res.columns, vec!["id", "label"]);
        assert_eq!(res.rows, vec![vec!["1", "a"], vec!["2", "b"]]);

        let ddl = db.introspect_schema().await.unwrap();
        assert_eq!(ddl.len(), 1);
        assert!(ddl[0].contains("CREATE TABLE t"));
        assert!(ddl[0].contains("id"));
        assert!(ddl[0].contains("label"));
    }

    #[tokio::test]
    async fn aggregates_return_numeric_cells() {
        let db = DuckDbRunner::new(":memory:").unwrap();
        db.run_sql("CREATE TABLE n (v INTEGER)").await.unwrap();
        db.run_sql("INSERT INTO n VALUES (10), (20), (30)").await.unwrap();
        let res = db.run_sql("SELECT SUM(v) AS total, COUNT(*) AS c FROM n").await.unwrap();
        assert_eq!(res.rows, vec![vec!["60", "3"]]);
    }

    #[tokio::test]
    async fn queries_a_csv_file_directly() {
        // DuckDB's headline personal-data feature: SELECT straight from a file.
        let csv = std::env::temp_dir().join(format!("opendbpylot_duck_{}.csv", std::process::id()));
        std::fs::write(&csv, "city,sales\nParis,10\nLyon,5\nParis,7\n").unwrap();

        let db = DuckDbRunner::new(":memory:").unwrap();
        let sql = format!(
            "SELECT city, SUM(sales) AS total FROM '{}' GROUP BY city ORDER BY total DESC",
            csv.to_string_lossy()
        );
        let res = db.run_sql(&sql).await.unwrap();
        assert_eq!(res.columns, vec!["city", "total"]);
        assert_eq!(res.rows, vec![vec!["Paris", "17"], vec!["Lyon", "5"]]);

        let _ = std::fs::remove_file(&csv);
    }
}
