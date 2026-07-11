//! Cross-backend integration smoke tests: for every supported database, prove
//! the three things a user cares about — **connect**, **learn the schema**,
//! **answer a question with correct rows** (via the engine + a scripted LLM).
//!
//! - SQLite runs unconditionally.
//! - DuckDB runs with `--features duckdb`.
//! - PostgreSQL / MySQL need a live server; they run only when the matching env
//!   var is set (and skip loudly otherwise), so plain `cargo test` stays green:
//!
//! ```bash
//! docker run -d --rm --name odp-pg -e POSTGRES_USER=opendb -e POSTGRES_PASSWORD=secret \
//!   -e POSTGRES_DB=testdb -p 5432:5432 postgres:16-alpine
//! docker run -d --rm --name odp-my -e MYSQL_ROOT_PASSWORD=secret -e MYSQL_DATABASE=testdb \
//!   -e MYSQL_USER=opendb -e MYSQL_PASSWORD=secret -p 3306:3306 mysql:8
//!
//! OPENDBPYLOT_TEST_POSTGRES_URL='postgres://opendb:secret@127.0.0.1:5432/testdb' \
//! OPENDBPYLOT_TEST_MYSQL_URL='mysql://opendb:secret@127.0.0.1:3306/testdb' \
//! cargo test --test backends
//! ```

use std::sync::Arc;

use opendbpylot::embedding::local::LocalEmbedding;
use opendbpylot::llm::mock::ScriptedMockLlm;
use opendbpylot::opendbpylot::{OpenDbPylot, OpenDbPylotConfig};
use opendbpylot::sqlrunner::SqlRunner;
use opendbpylot::vectorstore::memory::MemoryVectorStore;

/// The shared per-backend flow. `seed` statements run raw on the runner
/// (CREATE/INSERT are allowed there — the read-only gate applies to the LLM
/// path, not to setup code). The scripted LLM then "answers" with `select_sql`,
/// and we assert the returned rows and that the schema was actually learned.
async fn backend_flow(runner: Arc<dyn SqlRunner>, dialect: &str, seed: &[&str], select_sql: &str) {
    // 1. Connect.
    runner
        .run_sql("SELECT 1")
        .await
        .unwrap_or_else(|e| panic!("[{dialect}] failed to connect: {e:#}"));

    // 2. Seed (idempotent: drop first).
    for stmt in seed {
        runner
            .run_sql(stmt)
            .await
            .unwrap_or_else(|e| panic!("[{dialect}] seed failed on `{stmt}`: {e:#}"));
    }

    // 3. Engine with a scripted LLM (returns our SELECT).
    let llm = Arc::new(ScriptedMockLlm::new(vec![select_sql.to_string()]));
    let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
    let bot = OpenDbPylot::new(llm, store)
        .with_runner(runner.clone())
        .with_config(OpenDbPylotConfig { dialect: dialect.into(), ..Default::default() });

    // 4. Learn the schema ("Import schema" path).
    let learned = bot
        .train_from_schema()
        .await
        .unwrap_or_else(|e| panic!("[{dialect}] train_from_schema failed: {e:#}"));
    assert!(learned >= 1, "[{dialect}] expected at least 1 table learned, got {learned}");
    let ddl = bot.list_ddl().await.unwrap();
    assert!(
        ddl.iter().any(|d| d.to_lowercase().contains("opendbpylot_smoke")),
        "[{dialect}] learned DDL should mention our table, got: {ddl:?}"
    );

    // 5. Ask → correct rows back (schema validation + read-only gate + execution).
    let out = bot
        .ask("what are the names?")
        .await
        .unwrap_or_else(|e| panic!("[{dialect}] ask failed: {e:#}"));
    let rows = out.result.unwrap_or_else(|| panic!("[{dialect}] no result rows"));
    assert_eq!(
        rows.rows,
        vec![vec!["Ana".to_string()], vec!["Bo".to_string()]],
        "[{dialect}] wrong rows"
    );
    assert_eq!(out.repairs_used, 0, "[{dialect}] should not need repairs");

    // 6. Cleanup.
    let _ = runner.run_sql("DROP TABLE opendbpylot_smoke").await;
}

#[tokio::test]
async fn sqlite_end_to_end() {
    let path = std::env::temp_dir().join(format!("odp_backend_sqlite_{}.db", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let runner = Arc::new(opendbpylot::sqlrunner::sqlite::SqliteRunner::new(
        path.to_string_lossy().to_string(),
    ));
    backend_flow(
        runner,
        "SQLite",
        &[
            "DROP TABLE IF EXISTS opendbpylot_smoke",
            "CREATE TABLE opendbpylot_smoke (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            "INSERT INTO opendbpylot_smoke VALUES (1,'Ana'),(2,'Bo')",
        ],
        "SELECT name FROM opendbpylot_smoke ORDER BY id;",
    )
    .await;
    let _ = std::fs::remove_file(&path);
}

#[cfg(feature = "duckdb")]
#[tokio::test]
async fn duckdb_end_to_end() {
    let runner =
        Arc::new(opendbpylot::sqlrunner::duckdb::DuckDbRunner::new(":memory:").expect("open duckdb"));
    backend_flow(
        runner.clone(),
        "DuckDB",
        &[
            "DROP TABLE IF EXISTS opendbpylot_smoke",
            "CREATE TABLE opendbpylot_smoke (id INTEGER PRIMARY KEY, name VARCHAR NOT NULL)",
            "INSERT INTO opendbpylot_smoke VALUES (1,'Ana'),(2,'Bo')",
        ],
        "SELECT name FROM opendbpylot_smoke ORDER BY id;",
    )
    .await;

    // DuckDB extra: query a CSV file directly through the full ask path.
    let csv = std::env::temp_dir().join(format!("odp_backend_{}.csv", std::process::id()));
    std::fs::write(&csv, "city,sales\nParis,10\nLyon,5\n").unwrap();
    let llm = Arc::new(ScriptedMockLlm::new(vec![format!(
        "SELECT city FROM '{}' ORDER BY sales DESC;",
        csv.to_string_lossy()
    )]));
    let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
    let bot = OpenDbPylot::new(llm, store)
        .with_runner(runner)
        .with_config(OpenDbPylotConfig { dialect: "DuckDB".into(), ..Default::default() });
    let out = bot.ask("cities by sales?").await.expect("csv ask");
    assert_eq!(out.result.unwrap().rows, vec![vec!["Paris"], vec!["Lyon"]]);
    let _ = std::fs::remove_file(&csv);
}

#[cfg(feature = "remote-db")]
#[tokio::test]
async fn postgres_end_to_end() {
    let Ok(url) = std::env::var("OPENDBPYLOT_TEST_POSTGRES_URL") else {
        eprintln!("SKIPPED: set OPENDBPYLOT_TEST_POSTGRES_URL to run the PostgreSQL smoke test");
        return;
    };
    let runner = Arc::new(opendbpylot::sqlrunner::postgres::PostgresRunner::new(url));
    backend_flow(
        runner,
        "PostgreSQL",
        &[
            "DROP TABLE IF EXISTS opendbpylot_smoke",
            "CREATE TABLE opendbpylot_smoke (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            "INSERT INTO opendbpylot_smoke VALUES (1,'Ana'),(2,'Bo')",
        ],
        "SELECT name FROM opendbpylot_smoke ORDER BY id;",
    )
    .await;
}

#[cfg(feature = "remote-db")]
#[tokio::test]
async fn mysql_end_to_end() {
    let Ok(url) = std::env::var("OPENDBPYLOT_TEST_MYSQL_URL") else {
        eprintln!("SKIPPED: set OPENDBPYLOT_TEST_MYSQL_URL to run the MySQL smoke test");
        return;
    };
    let runner = Arc::new(opendbpylot::sqlrunner::mysql::MySqlRunner::new(url));
    backend_flow(
        runner,
        "MySQL",
        &[
            "DROP TABLE IF EXISTS opendbpylot_smoke",
            "CREATE TABLE opendbpylot_smoke (id INTEGER PRIMARY KEY, name VARCHAR(50) NOT NULL)",
            "INSERT INTO opendbpylot_smoke VALUES (1,'Ana'),(2,'Bo')",
        ],
        "SELECT name FROM opendbpylot_smoke ORDER BY id;",
    )
    .await;
}

/// Unreachable servers must fail with a clean, fast error — not hang.
#[cfg(feature = "remote-db")]
#[tokio::test]
async fn unreachable_postgres_fails_fast_and_clean() {
    let runner = Arc::new(opendbpylot::sqlrunner::postgres::PostgresRunner::new(
        // Reserved TEST-NET address — guaranteed unroutable.
        "postgres://u:p@192.0.2.1:5432/nope",
    ));
    let started = std::time::Instant::now();
    let err = runner.run_sql("SELECT 1").await.unwrap_err();
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "must fail fast");
    let msg = format!("{err:#}").to_lowercase();
    assert!(msg.contains("time") || msg.contains("connect"), "unhelpful error: {msg}");
}
