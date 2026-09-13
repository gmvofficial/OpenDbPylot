//! Shared demo setup used by both the CLI and the server, so they stay in sync.
//!
//! Builds a realistic little e-commerce database (~7k rows across customers,
//! products, orders, order_items) so the product demo can show real charts and
//! tables. Data is generated deterministically (a fixed-seed LCG) so every build
//! produces the exact same database — reproducible demos.

use std::sync::Arc;

use anyhow::Result;

use crate::conversation::MemoryConversationStore;
use crate::embedding::{
    cache::CachedEmbedding, local::LocalEmbedding, openai::OpenAiEmbedding, EmbeddingService,
};
use crate::llm::{mock::MockLlm, openai::OpenAiLlm, retry::RetryLlm, LlmService};
use crate::sqlrunner::{sqlite::SqliteRunner, SqlRunner};
use crate::opendbpylot::{OpenDbPylot, OpenDbPylotConfig};
use crate::vectorstore::memory::MemoryVectorStore;
use crate::vectorstore::VectorStore;

/// Pick the vector store. If `OPENDBPYLOT_QDRANT_URL` is set and the `qdrant` feature is
/// enabled, use Qdrant; otherwise use the in-memory store.
pub async fn build_vector_store(embedding: Arc<dyn EmbeddingService>) -> Result<Arc<dyn VectorStore>> {
    #[cfg(feature = "qdrant")]
    {
        if let Ok(url) = std::env::var("OPENDBPYLOT_QDRANT_URL") {
            let collection =
                std::env::var("OPENDBPYLOT_QDRANT_COLLECTION").unwrap_or_else(|_| "opendbpylot".into());
            let store =
                crate::vectorstore::qdrant::QdrantVectorStore::new(&url, collection, embedding).await?;
            return Ok(Arc::new(store));
        }
    }
    Ok(Arc::new(MemoryVectorStore::new(embedding)))
}

/// Choose LLM + embedding providers from the environment. Returns a label too.
pub fn pick_providers() -> (Arc<dyn LlmService>, Arc<dyn EmbeddingService>, &'static str) {
    match std::env::var("OPENAI_API_KEY") {
        Ok(key) if !key.is_empty() => (
            // Retries transient failures (timeouts, 429, 5xx) with backoff.
            Arc::new(RetryLlm::new(Arc::new(OpenAiLlm::new(key.clone(), "gpt-4o-mini")))),
            // File-backed cache: identical texts are only ever embedded (paid) once.
            Arc::new(CachedEmbedding::new(
                Arc::new(OpenAiEmbedding::new(key, "text-embedding-3-small")),
                "openai:text-embedding-3-small",
                Some(crate::app::home().join("cache").join("embeddings.jsonl")),
            )),
            "OpenAI",
        ),
        _ => (
            Arc::new(MockLlm::with_default_sql()),
            Arc::new(LocalEmbedding::new()),
            "offline mock",
        ),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Deterministic data generation
// ─────────────────────────────────────────────────────────────────────────────

const N_CUSTOMERS: usize = 400;
const N_ORDERS: usize = 2000;

/// A tiny seeded linear-congruential RNG (reproducible, no external dep).
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 1
    }
    fn range(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn range_incl(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % ((hi - lo + 1) as u64)) as i64
    }
}

const FIRST_NAMES: &[&str] = &[
    "James", "Mary", "John", "Patricia", "Robert", "Jennifer", "Michael", "Linda", "David",
    "Elizabeth", "William", "Barbara", "Richard", "Susan", "Joseph", "Jessica", "Thomas", "Sarah",
    "Charles", "Karen", "Maria", "Carlos", "Yuki", "Hans", "Sophie", "Liam", "Olivia", "Noah",
    "Emma", "Lucas", "Ava", "Aarav", "Priya", "Chen", "Wei", "Ana", "Pedro", "Sofia",
];

const LAST_NAMES: &[&str] = &[
    "Smith", "Johnson", "Williams", "Brown", "Jones", "Garcia", "Miller", "Davis", "Rodriguez",
    "Martinez", "Hernandez", "Lopez", "Gonzalez", "Wilson", "Anderson", "Taylor", "Moore",
    "Jackson", "Martin", "Lee", "Patel", "Kim", "Nakamura", "Muller", "Dubois", "Rossi", "Silva",
    "Santos", "Costa", "Schmidt",
];

/// (country, city) pairs — picking one keeps the city consistent with its country.
const PLACES: &[(&str, &str)] = &[
    ("USA", "New York"), ("USA", "San Francisco"), ("USA", "Chicago"), ("USA", "Austin"),
    ("UK", "London"), ("UK", "Manchester"),
    ("Germany", "Berlin"), ("Germany", "Munich"),
    ("France", "Paris"), ("France", "Lyon"),
    ("Canada", "Toronto"), ("Canada", "Vancouver"),
    ("Australia", "Sydney"), ("Australia", "Melbourne"),
    ("India", "Mumbai"), ("India", "Bangalore"),
    ("Brazil", "Sao Paulo"), ("Brazil", "Rio de Janeiro"),
    ("Japan", "Tokyo"), ("Japan", "Osaka"),
    ("Spain", "Madrid"), ("Spain", "Barcelona"),
];

/// (name, category, price) for the product catalog.
const PRODUCTS: &[(&str, &str, f64)] = &[
    ("Wireless Headphones", "Electronics", 129.99),
    ("4K Monitor", "Electronics", 329.00),
    ("Mechanical Keyboard", "Electronics", 89.50),
    ("USB-C Hub", "Electronics", 39.99),
    ("Bluetooth Speaker", "Electronics", 59.99),
    ("Smartphone Stand", "Electronics", 19.99),
    ("1080p Webcam", "Electronics", 49.99),
    ("Gaming Mouse", "Electronics", 45.00),
    ("Coffee Maker", "Home & Kitchen", 79.99),
    ("Air Fryer", "Home & Kitchen", 119.99),
    ("Knife Set", "Home & Kitchen", 64.50),
    ("Blender", "Home & Kitchen", 49.99),
    ("Cookware Set", "Home & Kitchen", 149.00),
    ("Vacuum Cleaner", "Home & Kitchen", 199.99),
    ("Rust Programming", "Books", 39.99),
    ("Data Science Handbook", "Books", 54.99),
    ("Mystery Novel", "Books", 14.99),
    ("Cookbook Deluxe", "Books", 29.99),
    ("Children's Picture Book", "Books", 12.99),
    ("Cotton T-Shirt", "Clothing", 19.99),
    ("Denim Jeans", "Clothing", 59.99),
    ("Running Jacket", "Clothing", 89.99),
    ("Wool Sweater", "Clothing", 74.50),
    ("Baseball Cap", "Clothing", 24.99),
    ("Yoga Mat", "Sports", 29.99),
    ("Dumbbell Set", "Sports", 89.99),
    ("Water Bottle", "Sports", 17.99),
    ("Tennis Racket", "Sports", 109.99),
    ("Soccer Ball", "Sports", 27.50),
    ("Building Blocks", "Toys", 49.99),
    ("Remote Control Car", "Toys", 39.99),
    ("1000pc Puzzle", "Toys", 18.99),
    ("Board Game", "Toys", 34.99),
    ("Face Cream", "Beauty", 34.99),
    ("Perfume", "Beauty", 79.99),
    ("Lipstick Set", "Beauty", 29.99),
    ("Hair Dryer", "Beauty", 44.99),
    ("Coffee Beans", "Grocery", 22.99),
    ("Olive Oil", "Grocery", 18.99),
    ("Dark Chocolate", "Grocery", 8.99),
    ("Green Tea", "Grocery", 12.99),
];

fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

fn rand_date(rng: &mut Rng, y0: i64, y1: i64) -> String {
    let year = rng.range_incl(y0, y1);
    let month = rng.range_incl(1, 12);
    let day = rng.range_incl(1, 28);
    format!("{year:04}-{month:02}-{day:02}")
}

fn weighted_status(rng: &mut Rng) -> &'static str {
    match rng.range(100) {
        0..=54 => "completed",
        55..=74 => "shipped",
        75..=84 => "pending",
        85..=92 => "cancelled",
        _ => "refunded",
    }
}

/// Insert rows in batches (one multi-row INSERT per ~200 rows).
async fn insert_batched(db: &SqliteRunner, table: &str, cols: &str, rows: &[String]) -> Result<()> {
    for chunk in rows.chunks(200) {
        let sql = format!("INSERT INTO {table} ({cols}) VALUES {}", chunk.join(","));
        db.run_sql(&sql).await?;
    }
    Ok(())
}

/// Create the demo SQLite database: a small e-commerce dataset (~7k rows).
pub async fn setup_demo_db(db: &SqliteRunner) -> Result<()> {
    // Fresh start (drop legacy + current tables).
    for t in ["users", "order_items", "orders", "products", "customers"] {
        db.run_sql(&format!("DROP TABLE IF EXISTS {t}")).await?;
    }

    db.run_sql(
        "CREATE TABLE customers (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            country TEXT NOT NULL,
            city TEXT NOT NULL,
            signup_date TEXT NOT NULL
        )",
    )
    .await?;
    db.run_sql(
        "CREATE TABLE products (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            category TEXT NOT NULL,
            price REAL NOT NULL
        )",
    )
    .await?;
    db.run_sql(
        "CREATE TABLE orders (
            id INTEGER PRIMARY KEY,
            customer_id INTEGER NOT NULL,
            order_date TEXT NOT NULL,
            status TEXT NOT NULL
        )",
    )
    .await?;
    db.run_sql(
        "CREATE TABLE order_items (
            id INTEGER PRIMARY KEY,
            order_id INTEGER NOT NULL,
            product_id INTEGER NOT NULL,
            quantity INTEGER NOT NULL,
            unit_price REAL NOT NULL
        )",
    )
    .await?;

    let mut rng = Rng::new(0x5EED_C0DE);

    // Customers (ids auto-assigned 1..=N_CUSTOMERS in insertion order).
    let mut customers = Vec::with_capacity(N_CUSTOMERS);
    for _ in 0..N_CUSTOMERS {
        let fname = FIRST_NAMES[rng.range(FIRST_NAMES.len())];
        let lname = LAST_NAMES[rng.range(LAST_NAMES.len())];
        let (country, city) = PLACES[rng.range(PLACES.len())];
        let signup = rand_date(&mut rng, 2022, 2024);
        customers.push(format!(
            "('{} {}','{}','{}','{}')",
            esc(fname), esc(lname), esc(country), esc(city), signup
        ));
    }
    insert_batched(db, "customers", "name, country, city, signup_date", &customers).await?;

    // Products (ids auto 1..=PRODUCTS.len()).
    let prices: Vec<f64> = PRODUCTS.iter().map(|(_, _, p)| *p).collect();
    let products: Vec<String> = PRODUCTS
        .iter()
        .map(|(name, cat, price)| format!("('{}','{}',{:.2})", esc(name), esc(cat), price))
        .collect();
    insert_batched(db, "products", "name, category, price", &products).await?;

    // Orders (ids auto 1..=N_ORDERS).
    let mut orders = Vec::with_capacity(N_ORDERS);
    for _ in 0..N_ORDERS {
        let customer = rng.range(N_CUSTOMERS) + 1;
        let date = rand_date(&mut rng, 2023, 2024);
        let status = weighted_status(&mut rng);
        orders.push(format!("({},'{}','{}')", customer, date, status));
    }
    insert_batched(db, "orders", "customer_id, order_date, status", &orders).await?;

    // Order items: 1–4 per order.
    let mut items = Vec::with_capacity(N_ORDERS * 3);
    for order_id in 1..=N_ORDERS {
        let line_count = rng.range(4) + 1;
        for _ in 0..line_count {
            let pid = rng.range(prices.len()) + 1;
            let qty = rng.range(5) + 1;
            let price = prices[pid - 1];
            items.push(format!("({},{},{},{:.2})", order_id, pid, qty, price));
        }
    }
    insert_batched(db, "order_items", "order_id, product_id, quantity, unit_price", &items).await?;

    Ok(())
}

/// Train a `OpenDbPylot`/`Agent` knowledge base on the demo schema: DDL, business notes,
/// and example question→SQL pairs (several chart-friendly). Shared by the CLI and
/// the server so they stay in sync.
pub async fn train_demo(opendbpylot: &OpenDbPylot) -> Result<()> {
    // ── DDL ──
    opendbpylot.train_ddl(
        "CREATE TABLE customers (id INTEGER PRIMARY KEY, name TEXT, country TEXT, city TEXT, signup_date TEXT);",
    ).await?;
    opendbpylot.train_ddl(
        "CREATE TABLE products (id INTEGER PRIMARY KEY, name TEXT, category TEXT, price REAL);",
    ).await?;
    opendbpylot.train_ddl(
        "CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER, order_date TEXT, status TEXT);",
    ).await?;
    opendbpylot.train_ddl(
        "CREATE TABLE order_items (id INTEGER PRIMARY KEY, order_id INTEGER, product_id INTEGER, quantity INTEGER, unit_price REAL);",
    ).await?;

    // ── Business documentation ──
    opendbpylot.train_documentation(
        "This is an e-commerce database. customers place orders; each order has many order_items; \
         each order_item refers to a product. Join order_items.order_id = orders.id, \
         order_items.product_id = products.id, and orders.customer_id = customers.id.",
    ).await?;
    opendbpylot.train_documentation(
        "Revenue is order_items.quantity * order_items.unit_price. For realized revenue, exclude \
         orders whose status is 'cancelled' or 'refunded'.",
    ).await?;
    opendbpylot.train_documentation(
        "orders.status is one of: completed, shipped, pending, cancelled, refunded. \
         products.category is one of: Electronics, Home & Kitchen, Books, Clothing, Sports, Toys, Beauty, Grocery.",
    ).await?;
    opendbpylot.train_documentation(
        "order_date and signup_date are text dates in 'YYYY-MM-DD' format. To group by month use \
         substr(order_date, 1, 7) (gives 'YYYY-MM').",
    ).await?;

    // ── Example question → SQL pairs ──
    opendbpylot.train_question_sql(
        "What is the total revenue by product category?",
        "SELECT p.category, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue \
         FROM order_items oi \
         JOIN products p ON p.id = oi.product_id \
         JOIN orders o ON o.id = oi.order_id \
         WHERE o.status NOT IN ('cancelled','refunded') \
         GROUP BY p.category ORDER BY revenue DESC;",
    ).await?;
    opendbpylot.train_question_sql(
        "How many orders are there per country?",
        "SELECT c.country, COUNT(*) AS orders \
         FROM orders o JOIN customers c ON c.id = o.customer_id \
         GROUP BY c.country ORDER BY orders DESC;",
    ).await?;
    opendbpylot.train_question_sql(
        "Show monthly revenue over time",
        "SELECT substr(o.order_date, 1, 7) AS month, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue \
         FROM orders o JOIN order_items oi ON oi.order_id = o.id \
         WHERE o.status NOT IN ('cancelled','refunded') \
         GROUP BY month ORDER BY month;",
    ).await?;
    opendbpylot.train_question_sql(
        "What are the top 10 products by revenue?",
        "SELECT p.name, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue \
         FROM order_items oi JOIN products p ON p.id = oi.product_id \
         GROUP BY p.name ORDER BY revenue DESC LIMIT 10;",
    ).await?;
    opendbpylot.train_question_sql(
        "Break down orders by status",
        "SELECT status, COUNT(*) AS orders FROM orders GROUP BY status ORDER BY orders DESC;",
    ).await?;

    Ok(())
}

/// Build a fully trained demo `OpenDbPylot` connected to the demo database.
pub async fn build_demo_opendbpylot() -> Result<(OpenDbPylot, &'static str)> {
    let (opendbpylot, backend, _db) = build_demo_with_runner(true).await?;
    Ok((opendbpylot, backend))
}

/// As [`build_demo_opendbpylot`], but also hands back the runner and lets the
/// caller turn off self-training.
///
/// The benchmark needs both: the runner to execute reference SQL, and
/// `auto_train = false` so a passing case cannot teach itself to a later one —
/// which would make the score depend on case order and stop it being
/// reproducible.
pub async fn build_demo_with_runner(
    auto_train: bool,
) -> Result<(OpenDbPylot, &'static str, Arc<SqliteRunner>)> {
    build_demo_at(auto_train, "demo.db").await
}

/// As [`build_demo_with_runner`], against a database file of the caller's
/// choosing.
///
/// The benchmark uses a temporary path: `demo.db` is tracked in the repository,
/// and regenerating it on every eval run leaves a dirty working tree that is
/// easy to commit by accident.
pub async fn build_demo_at(
    auto_train: bool,
    db_path: &str,
) -> Result<(OpenDbPylot, &'static str, Arc<SqliteRunner>)> {
    let (llm, embedding, backend) = pick_providers();

    let db = SqliteRunner::new(db_path.to_string());
    setup_demo_db(&db).await?;
    let db = Arc::new(db);

    let store = build_vector_store(embedding).await?;
    // Demo data is local & non-sensitive, so we let the model peek at values
    // (enables the intermediate_sql path for value-dependent questions).
    let opendbpylot = OpenDbPylot::new(llm, store)
        .with_runner(db.clone())
        .with_conversations(Arc::new(MemoryConversationStore::new()))
        .with_config(OpenDbPylotConfig {
            dialect: "SQLite".into(),
            auto_train,
            allow_llm_to_see_data: true,
            // Opt-in NL answer above the table. Off for the offline mock (whose
            // canned summary adds nothing); on for real providers when the user
            // sets OPENDBPYLOT_SUMMARIZE, so we don't silently double their spend.
            summarize_results: backend != "offline mock" && std::env::var("OPENDBPYLOT_SUMMARIZE").is_ok(),
            ..Default::default()
        });

    train_demo(&opendbpylot).await?;

    Ok((opendbpylot, backend, db))
}
