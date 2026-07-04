//! One-shot generator for the sample demo database.
//!
//!   cargo run --bin gen_demo            # writes ./demo.db
//!   cargo run --bin gen_demo my.db      # writes ./my.db
//!
//! The demo database is just a sample SQLite file — once generated you connect to
//! it from the app exactly like any other database (Settings → it auto-imports the
//! schema). It is NOT special-cased at runtime.

use anyhow::Result;

use opendbpylot::demo::setup_demo_db;
use opendbpylot::sqlrunner::sqlite::SqliteRunner;

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "demo.db".to_string());
    println!("Generating sample e-commerce database at {path} …");
    setup_demo_db(&SqliteRunner::new(path.clone())).await?;

    let db = SqliteRunner::new(path.clone());
    use opendbpylot::sqlrunner::SqlRunner;
    for table in ["customers", "products", "orders", "order_items"] {
        let r = db.run_sql(&format!("SELECT COUNT(*) FROM {table}")).await?;
        let count = r.rows.first().and_then(|row| row.first()).cloned().unwrap_or_default();
        println!("  {table:<12} {count} rows");
    }
    println!("Done. Connect to '{path}' from the app to import its schema and start querying.");
    Ok(())
}
