//! Agent-memory tools — let the model save durable knowledge so answers get more
//! consistent over time (opendbpylot-main's `AgentMemory` idea, adapted).
//!
//! Two things the model can save:
//! - `remember`            → a durable fact/definition/correction (stored as documentation)
//! - `save_query_example`  → a validated question→SQL pair (stored as a few-shot example)
//!
//! Retrieval is **automatic**: the `RagEnhancer` injects the most relevant saved
//! facts and examples into the system prompt each turn, so there is no extra
//! round-trip (unlike opendbpylot-main's mandatory "search before every tool"). Both
//! memory kinds are stored in the same per-database vector store as the schema, so
//! they survive schema re-imports (which only clear DDL).

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::core::tool::{Tool, ToolContext, ToolResult};
use crate::sql::is_read_only;
use crate::vectorstore::VectorStore;

/// `remember` — save a durable domain fact / definition / correction.
pub struct RememberTool {
    store: Arc<dyn VectorStore>,
}

impl RememberTool {
    pub fn new(store: Arc<dyn VectorStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for RememberTool {
    fn name(&self) -> &str {
        "remember"
    }

    fn description(&self) -> &str {
        "Save a durable fact, definition, or correction about the user's data for future \
         questions — e.g. \"active means status = 1\" or \"always exclude rows where email \
         contains 'test'\". Use this only for lasting domain knowledge the user teaches you, \
         not for one-off details."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "fact": { "type": "string", "description": "The durable fact or rule to remember." }
            },
            "required": ["fact"]
        })
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let fact = args["fact"].as_str().unwrap_or("").trim();
        if fact.is_empty() {
            return Ok(ToolResult::error("Nothing to remember (empty fact)."));
        }
        self.store.add_documentation(fact).await?;
        Ok(ToolResult::ok(format!("Saved to memory: {fact}")))
    }
}

/// `save_query_example` — save a validated question→SQL pair for consistency.
pub struct SaveQueryTool {
    store: Arc<dyn VectorStore>,
}

impl SaveQueryTool {
    pub fn new(store: Arc<dyn VectorStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for SaveQueryTool {
    fn name(&self) -> &str {
        "save_query_example"
    }

    fn description(&self) -> &str {
        "Save a question and the correct SQL that answers it, so similar future questions are \
         answered consistently. Call this ONLY when the user asks you to remember a query, or \
         after you have corrected a query to be right — never for every routine question."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The natural-language question." },
                "sql": { "type": "string", "description": "The correct, read-only SQL that answers it." }
            },
            "required": ["question", "sql"]
        })
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let question = args["question"].as_str().unwrap_or("").trim();
        let sql = args["sql"].as_str().unwrap_or("").trim();
        if question.is_empty() || sql.is_empty() {
            return Ok(ToolResult::error("Need both a question and its SQL to save an example."));
        }
        // Never memorize a non-read-only query.
        if !is_read_only(sql) {
            return Ok(ToolResult::error("Refusing to save a non-read-only query as an example."));
        }
        self.store.add_question_sql(question, sql).await?;
        Ok(ToolResult::ok(format!("Saved example for: {question}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::enhancer::{ContextEnhancer, RagEnhancer};
    use crate::embedding::local::LocalEmbedding;
    use crate::vectorstore::memory::MemoryVectorStore;

    fn store() -> Arc<dyn VectorStore> {
        Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())))
    }

    #[tokio::test]
    async fn remember_saves_and_is_recalled_by_the_enhancer() {
        let store = store();
        let tool = RememberTool::new(store.clone());

        let res = tool
            .execute(&ToolContext::default(), json!({ "fact": "active users are those with status = 1" }))
            .await
            .unwrap();
        assert!(res.success);

        // The RagEnhancer must surface it for a related question (auto-recall).
        let enhancer = RagEnhancer::new(store.clone());
        let ctx = enhancer.enhance("how many active users are there").await;
        assert!(ctx.contains("status = 1"), "saved fact should be injected: {ctx}");
    }

    #[tokio::test]
    async fn save_query_example_stores_reads_and_rejects_writes() {
        let store = store();
        let tool = SaveQueryTool::new(store.clone());

        // A read-only query is saved.
        let ok = tool
            .execute(
                &ToolContext::default(),
                json!({ "question": "count users", "sql": "SELECT COUNT(*) FROM users" }),
            )
            .await
            .unwrap();
        assert!(ok.success);
        assert_eq!(store.all_question_sql().await.unwrap().len(), 1);

        // A destructive query is refused.
        let bad = tool
            .execute(
                &ToolContext::default(),
                json!({ "question": "wipe", "sql": "DROP TABLE users" }),
            )
            .await
            .unwrap();
        assert!(!bad.success);
        assert_eq!(store.all_question_sql().await.unwrap().len(), 1); // unchanged
    }

    /// Quality guard: registering the memory tools must NOT change the normal
    /// query flow. The agent should still run the query and answer, not detour
    /// into memory tools.
    #[tokio::test]
    async fn memory_tools_do_not_change_the_normal_query_flow() {
        use crate::capabilities::file_system::MemoryFileSystem;
        use crate::capabilities::file_system::FileSystem;
        use crate::core::agent::{Agent, AgentEvent};
        use crate::core::registry::ToolRegistry;
        use crate::llm::mock::MockLlm;
        use crate::sqlrunner::sqlite::SqliteRunner;
        use crate::sqlrunner::SqlRunner;
        use crate::tools::run_sql::RunSqlTool;
        use crate::tools::visualize_data::VisualizeDataTool;
        use tokio_stream::StreamExt;

        // Temp DB with the demo shape the mock queries.
        let path = std::env::temp_dir().join(format!("opendbpylot_mem_{}.db", rand::random::<u32>()));
        let db = SqliteRunner::new(path.to_string_lossy().to_string());
        db.run_sql("CREATE TABLE products (id INTEGER, category TEXT, price REAL)").await.unwrap();
        db.run_sql("CREATE TABLE orders (id INTEGER, status TEXT)").await.unwrap();
        db.run_sql("CREATE TABLE order_items (order_id INTEGER, product_id INTEGER, quantity INTEGER, unit_price REAL)").await.unwrap();
        db.run_sql("INSERT INTO products VALUES (1,'Books',10.0)").await.unwrap();
        db.run_sql("INSERT INTO orders VALUES (1,'completed')").await.unwrap();
        db.run_sql("INSERT INTO order_items VALUES (1,1,2,10.0)").await.unwrap();

        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let st = store();

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(RunSqlTool::new(Arc::new(db), fs.clone())));
        registry.register(Arc::new(VisualizeDataTool::new(fs)));
        registry.register(Arc::new(RememberTool::new(st.clone())));
        registry.register(Arc::new(SaveQueryTool::new(st.clone())));

        let agent = Agent::new(Arc::new(MockLlm::with_default_sql()), Arc::new(registry));
        let stream = agent.send_message(ToolContext::default(), "revenue by category".into());
        tokio::pin!(stream);
        let mut names = Vec::new();
        while let Some(ev) = stream.next().await {
            if let AgentEvent::ToolStarted { name, .. } = ev {
                names.push(name);
            }
        }

        // It ran the query (and charted) — and did NOT wander into memory tools.
        assert!(names.contains(&"run_sql".to_string()));
        assert!(!names.contains(&"remember".to_string()));
        assert!(!names.contains(&"save_query_example".to_string()));

        let _ = std::fs::remove_file(&path);
    }
}
