//! Phase-0 mini eval harness: measures end-to-end NL→SQL accuracy on the demo DB.
//!
//! Usage:
//!   cargo run --example eval                     # real measurement (OPENAI_API_KEY in env/.env)
//!   OPENAI_API_KEY= cargo run --example eval     # offline plumbing check (mock LLM)
//!
//! Scoring is **execution accuracy**: the generated SQL and the hand-written
//! reference SQL are both executed; a case passes when the result rows match.
//! Row order is ignored and numbers are normalized to 2 decimal places, so
//! different-but-equivalent SQL still counts as correct.
//!
//!   strict  — rows equal with cells in the generated column order
//!   lenient — rows equal after sorting cells within each row (tolerates
//!             column-order and extra-alias differences that still carry
//!             the same values)
//!
//! The first five cases mirror the trained question→SQL examples (retrieval
//! should nail these); the rest are unseen questions of increasing difficulty.

use std::sync::Arc;

use opendbpylot::demo::{build_vector_store, pick_providers, setup_demo_db, train_demo};
use opendbpylot::opendbpylot::{OpenDbPylot, OpenDbPylotConfig};
use opendbpylot::sqlrunner::{sqlite::SqliteRunner, QueryResult, SqlRunner};

/// (question, reference SQL). References are hand-checked against the demo schema.
fn cases() -> Vec<(&'static str, &'static str)> {
    vec![
        // ── Seen during training (retrieval should carry these) ──
        (
            "What is the total revenue by product category? Exclude cancelled and refunded orders.",
            "SELECT p.category, SUM(oi.quantity * oi.unit_price) AS revenue
             FROM order_items oi
             JOIN products p ON p.id = oi.product_id
             JOIN orders o ON o.id = oi.order_id
             WHERE o.status NOT IN ('cancelled','refunded')
             GROUP BY p.category;",
        ),
        (
            "How many orders are there per country?",
            "SELECT c.country, COUNT(*) AS orders
             FROM orders o JOIN customers c ON c.id = o.customer_id
             GROUP BY c.country;",
        ),
        (
            "Show monthly revenue over time, excluding cancelled and refunded orders.",
            "SELECT substr(o.order_date, 1, 7) AS month, SUM(oi.quantity * oi.unit_price) AS revenue
             FROM orders o JOIN order_items oi ON oi.order_id = o.id
             WHERE o.status NOT IN ('cancelled','refunded')
             GROUP BY month;",
        ),
        (
            // "revenue" alone is ambiguous — the trained docs define *realized*
            // revenue as excluding cancelled/refunded, so say which one we mean.
            "What are the top 10 products by revenue? Count every order regardless of its status.",
            "SELECT p.name, SUM(oi.quantity * oi.unit_price) AS revenue
             FROM order_items oi JOIN products p ON p.id = oi.product_id
             GROUP BY p.name ORDER BY revenue DESC LIMIT 10;",
        ),
        (
            "Break down orders by status",
            "SELECT status, COUNT(*) AS orders FROM orders GROUP BY status;",
        ),
        // ── Unseen: simple lookups & counts ──
        (
            "How many customers are there in total?",
            "SELECT COUNT(*) FROM customers;",
        ),
        (
            "How many orders are there in total?",
            "SELECT COUNT(*) FROM orders;",
        ),
        (
            "How many products are in the Electronics category?",
            "SELECT COUNT(*) FROM products WHERE category = 'Electronics';",
        ),
        (
            "How many orders are currently pending?",
            "SELECT COUNT(*) FROM orders WHERE status = 'pending';",
        ),
        (
            "How many distinct product categories are there?",
            "SELECT COUNT(DISTINCT category) FROM products;",
        ),
        // ── Unseen: grouping & aggregation ──
        (
            "What is the average product price per category?",
            "SELECT category, AVG(price) FROM products GROUP BY category;",
        ),
        (
            "How many customers are in each country?",
            "SELECT country, COUNT(*) FROM customers GROUP BY country;",
        ),
        (
            "Which cities have customers? Show each city with its customer count.",
            "SELECT city, COUNT(*) FROM customers GROUP BY city;",
        ),
        (
            "How many orders were placed in each month of 2024?",
            "SELECT substr(order_date, 1, 7) AS month, COUNT(*)
             FROM orders WHERE order_date LIKE '2024-%' GROUP BY month;",
        ),
        (
            "How many customers signed up in 2023?",
            "SELECT COUNT(*) FROM customers WHERE signup_date LIKE '2023-%';",
        ),
        // ── Unseen: joins, filters, harder shapes ──
        (
            "What is the name and price of the most expensive product?",
            "SELECT name, price FROM products ORDER BY price DESC LIMIT 1;",
        ),
        (
            "What is the name and price of the cheapest product in the Books category?",
            "SELECT name, price FROM products WHERE category = 'Books' ORDER BY price ASC LIMIT 1;",
        ),
        (
            "How many units in total were sold per product category? Exclude cancelled and refunded orders.",
            "SELECT p.category, SUM(oi.quantity)
             FROM order_items oi
             JOIN products p ON p.id = oi.product_id
             JOIN orders o ON o.id = oi.order_id
             WHERE o.status NOT IN ('cancelled','refunded')
             GROUP BY p.category;",
        ),
        (
            "What was the total revenue in 2024, excluding cancelled and refunded orders?",
            "SELECT SUM(oi.quantity * oi.unit_price)
             FROM orders o JOIN order_items oi ON oi.order_id = o.id
             WHERE o.status NOT IN ('cancelled','refunded')
               AND o.order_date LIKE '2024-%';",
        ),
        (
            // Same ambiguity: force "all orders" explicitly so the model doesn't
            // (reasonably) apply the realized-revenue exclusion from the docs.
            "What is the average order value across all orders, including cancelled and refunded ones?",
            "SELECT AVG(t.total) FROM (
                SELECT SUM(oi.quantity * oi.unit_price) AS total
                FROM order_items oi GROUP BY oi.order_id
             ) t;",
        ),
    ]
}

/// Normalize one cell: parse-and-round numbers to 2 dp so `1234.5`, `1234.50`
/// and `1234.499999` all compare equal; leave non-numeric text as-is.
fn norm_cell(s: &str) -> String {
    match s.trim().parse::<f64>() {
        Ok(f) => format!("{f:.2}"),
        Err(_) => s.trim().to_string(),
    }
}

/// Order-insensitive row multiset, cells in column order (strict comparison).
fn norm_rows(r: &QueryResult) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = r
        .rows
        .iter()
        .map(|row| row.iter().map(|c| norm_cell(c)).collect())
        .collect();
    rows.sort();
    rows
}

/// Like `norm_rows`, but cells are sorted *within* each row too — tolerates
/// column-order differences at the cost of some false positives.
fn norm_rows_lenient(r: &QueryResult) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = r
        .rows
        .iter()
        .map(|row| {
            let mut cells: Vec<String> = row.iter().map(|c| norm_cell(c)).collect();
            cells.sort();
            cells
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let (llm, embedding, backend) = pick_providers();
    println!("provider: {backend}");
    if backend == "offline mock" {
        println!("NOTE: mock mode only checks harness plumbing — the score is not meaningful.\n");
    }

    let db = SqliteRunner::new("demo.db");
    setup_demo_db(&db).await?;
    let db = Arc::new(db);

    let store = build_vector_store(embedding).await?;
    let bot = OpenDbPylot::new(llm, store)
        .with_runner(db.clone())
        .with_config(OpenDbPylotConfig {
            dialect: "SQLite".into(),
            // Off so a passing case can't train itself into later cases —
            // keeps every run and every case independent.
            auto_train: false,
            allow_llm_to_see_data: true,
            ..Default::default()
        });
    train_demo(&bot).await?;

    let cases = cases();
    let total = cases.len();
    let (mut strict, mut lenient) = (0usize, 0usize);
    let started = std::time::Instant::now();

    for (i, (question, reference)) in cases.iter().enumerate() {
        let expected = db
            .run_sql(reference)
            .await
            .map_err(|e| anyhow::anyhow!("reference SQL for case {} is broken: {e}", i + 1))?;

        let (mark, detail) = match bot.ask(question).await {
            Ok(answer) => match &answer.result {
                Some(got) => {
                    let strict_ok = norm_rows(got) == norm_rows(&expected);
                    let lenient_ok =
                        strict_ok || norm_rows_lenient(got) == norm_rows_lenient(&expected);
                    if strict_ok {
                        strict += 1;
                    }
                    if lenient_ok {
                        lenient += 1;
                    }
                    let repairs = if answer.repairs_used > 0 {
                        format!(" ({} repair)", answer.repairs_used)
                    } else {
                        String::new()
                    };
                    match (strict_ok, lenient_ok) {
                        (true, _) => (format!("PASS {repairs}"), None),
                        (false, true) => (format!("pass~{repairs}"), None), // lenient only
                        (false, false) => ("FAIL ".to_string(), Some(answer.sql.clone())),
                    }
                }
                None => ("FAIL ".to_string(), Some(format!("not executed (not read-only?): {}", answer.sql))),
            },
            Err(e) => ("FAIL ".to_string(), Some(format!("error: {e}"))),
        };

        println!("#{:02} {} {}", i + 1, mark, question);
        if let Some(d) = detail {
            println!("        generated: {}", d.replace('\n', " "));
            println!("        reference: {}", reference.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    }

    println!(
        "\nscore: strict {strict}/{total}, lenient {lenient}/{total}  ({:.1}s, provider: {backend})",
        started.elapsed().as_secs_f32()
    );
    Ok(())
}
