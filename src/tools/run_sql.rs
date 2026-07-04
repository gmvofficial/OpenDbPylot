//! `RunSqlTool` — the product's main tool. Executes SQL via an injected
//! `SqlRunner`, streams the rows back as a dataframe UI payload, and (for SELECTs)
//! stashes the full result set in the `FileSystem` so `visualize_data` can chart it
//! without the data passing through the LLM's token budget.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::capabilities::file_system::FileSystem;
use crate::core::tool::{Tool, ToolContext, ToolResult};
use crate::sql::is_read_only;
use crate::sqlrunner::{QueryResult, SqlRunner};

/// How much of the result preview to feed the LLM (the rest lives in the file).
const PREVIEW_LIMIT: usize = 1000;

/// Max rows sent to the browser in the results table (keeps the payload small).
/// The full (runner-capped) set is still written to the file for charting.
const DISPLAY_LIMIT: usize = 1000;

pub struct RunSqlTool {
    runner: Arc<dyn SqlRunner>,
    fs: Arc<dyn FileSystem>,
}

impl RunSqlTool {
    pub fn new(runner: Arc<dyn SqlRunner>, fs: Arc<dyn FileSystem>) -> Self {
        Self { runner, fs }
    }
}

/// Render a result set as simple CSV (used only for the LLM-facing preview).
fn to_csv(r: &QueryResult) -> String {
    let mut out = r.columns.join(",");
    out.push('\n');
    for row in &r.rows {
        out.push_str(&row.join(","));
        out.push('\n');
    }
    out
}

#[async_trait]
impl Tool for RunSqlTool {
    fn name(&self) -> &str {
        "run_sql"
    }

    fn description(&self) -> &str {
        "Execute a read-only SQL query against the connected database and return the rows. \
         Use this to actually answer the user's data questions — do not just describe SQL."
    }

    fn args_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "sql": {
                    "type": "string",
                    "description": "The SQL query to execute (dialect of the connected database)."
                }
            },
            "required": ["sql"]
        })
    }

    async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult> {
        let sql = args["sql"].as_str().unwrap_or("").trim().to_string();
        if sql.is_empty() {
            return Ok(ToolResult::error("No SQL provided."));
        }

        // SECURITY GATE: only read-only queries may run. This is checked BEFORE
        // execution so a destructive statement never touches the database.
        if !is_read_only(&sql) {
            return Ok(ToolResult::error(
                "Blocked: only read-only queries (SELECT / WITH) are allowed. \
                 This tool cannot modify or delete data.",
            ));
        }

        let result = match self.runner.run_sql(&sql).await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(format!("SQL error: {e}"))),
        };

        if result.rows.is_empty() {
            let ui = json!({
                "kind": "dataframe",
                "columns": result.columns,
                "rows": [],
                "title": "Query Results",
                "description": "No rows returned"
            });
            return Ok(ToolResult::ok("Query executed successfully. No rows returned.").with_ui(ui));
        }

        // Stash the full result set (as JSON) for visualize_data to read back.
        let id = format!("{:08x}", rand::random::<u32>());
        let filename = format!("query_results_{id}.json");
        let structured = json!({ "columns": result.columns, "rows": result.rows });
        self.fs.write_file(&filename, &structured.to_string()).await?;

        // Truncated, LLM-facing preview.
        let mut preview = to_csv(&result);
        if preview.len() > PREVIEW_LIMIT {
            preview.truncate(PREVIEW_LIMIT);
            preview.push_str(
                "\n(Results truncated. For large result sets do NOT summarize them — \
                 call visualize_data next.)",
            );
        }
        let result_for_llm = format!(
            "{preview}\n\nResults saved to file: {filename}\n\n\
             **IMPORTANT: FOR VISUALIZE_DATA USE FILENAME: {filename}**"
        );

        // Cap the rows sent to the browser so the payload stays small; the full
        // (runner-capped) set is already saved to the file for charting.
        let total_rows = result.rows.len();
        let display_rows: Vec<Vec<String>> = result.rows.iter().take(DISPLAY_LIMIT).cloned().collect();
        let description = if total_rows > DISPLAY_LIMIT {
            format!("Showing first {DISPLAY_LIMIT} of {total_rows} rows, {} columns", result.columns.len())
        } else {
            format!("{total_rows} row(s), {} column(s)", result.columns.len())
        };

        let ui = json!({
            "kind": "dataframe",
            "columns": result.columns,
            "rows": display_rows,
            "title": "Query Results",
            "description": description
        });

        Ok(ToolResult::ok(result_for_llm).with_ui(ui))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::file_system::MemoryFileSystem;
    use crate::sqlrunner::sqlite::SqliteRunner;

    async fn temp_db() -> (Arc<SqliteRunner>, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("opendbpylot_runsql_{}.db", rand::random::<u32>()));
        let db = SqliteRunner::new(path.to_string_lossy().to_string());
        db.run_sql("CREATE TABLE users (id INTEGER, country TEXT)").await.unwrap();
        db.run_sql("INSERT INTO users VALUES (1,'USA'),(2,'USA'),(3,'UK')").await.unwrap();
        (Arc::new(db), path)
    }

    #[tokio::test]
    async fn select_returns_rows_and_stashes_a_file() {
        let (db, path) = temp_db().await;
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let tool = RunSqlTool::new(db, fs.clone());

        let res = tool
            .execute(
                &ToolContext::default(),
                json!({ "sql": "SELECT country, COUNT(*) AS n FROM users GROUP BY country ORDER BY n DESC" }),
            )
            .await
            .unwrap();

        assert!(res.success);
        // Preview mentions the data and a saved filename the model can pass on.
        assert!(res.result_for_llm.contains("USA"));
        assert!(res.result_for_llm.contains("query_results_"));

        // The UI payload is a dataframe with the rows.
        let ui = res.ui.expect("expected a dataframe ui payload");
        assert_eq!(ui["kind"], "dataframe");
        assert_eq!(ui["rows"].as_array().unwrap().len(), 2); // USA, UK

        // The stashed file actually exists and round-trips.
        let fname = ui_filename(&res.result_for_llm);
        let stored = fs.read_file(&fname).await.unwrap();
        assert!(stored.contains("country"));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn bad_sql_is_reported_not_panicked() {
        let (db, path) = temp_db().await;
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let tool = RunSqlTool::new(db, fs);

        let res = tool
            .execute(&ToolContext::default(), json!({ "sql": "SELECT * FROM nope" }))
            .await
            .unwrap();
        assert!(!res.success);
        assert!(res.result_for_llm.to_lowercase().contains("error"));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn destructive_sql_is_blocked_and_data_survives() {
        let (db, path) = temp_db().await;
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
        let tool = RunSqlTool::new(db.clone(), fs);

        // Attempt to drop the table.
        let res = tool
            .execute(&ToolContext::default(), json!({ "sql": "DROP TABLE users" }))
            .await
            .unwrap();

        // It must be blocked (not executed).
        assert!(!res.success);
        assert!(res.result_for_llm.to_lowercase().contains("read-only"));

        // The table must still be there with its rows.
        let check = db.run_sql("SELECT COUNT(*) FROM users").await.unwrap();
        assert_eq!(check.rows[0][0], "3");

        let _ = std::fs::remove_file(&path);
    }

    /// Pull the `query_results_*.json` filename out of the LLM-facing text.
    fn ui_filename(s: &str) -> String {
        s.split_whitespace()
            .find(|w| w.starts_with("query_results_"))
            .unwrap_or("")
            .trim_end_matches("**")
            .to_string()
    }

    /// Phase 3 acceptance: drive the full agent loop with a real RunSqlTool over a
    /// real SQLite db. The model "decides" to call run_sql with a real query, the
    /// query runs, and the loop returns a final answer.
    #[tokio::test]
    async fn agent_loop_runs_real_sql_end_to_end() {
        use crate::core::agent::{Agent, AgentEvent};
        use crate::core::registry::ToolRegistry;
        use crate::core::tool::ToolContext;
        use crate::llm::mock::ScriptedToolLlm;
        use crate::llm::{LlmResponse, ToolCall};
        use tokio_stream::StreamExt;

        let (db, path) = temp_db().await;
        let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(RunSqlTool::new(db, fs)));

        // Turn 1: model calls run_sql with a real query. Turn 2: final answer.
        let llm = Arc::new(ScriptedToolLlm::new(vec![
            LlmResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "run_sql".into(),
                    args: json!({ "sql": "SELECT country, COUNT(*) AS n FROM users GROUP BY country ORDER BY n DESC" }),
                }],
            },
            LlmResponse::text("The USA has the most users (2)."),
        ]));

        let agent = Agent::new(llm, Arc::new(registry));
        let stream = agent.send_message(ToolContext::default(), "which country has the most users?".into());
        tokio::pin!(stream);
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        // run_sql actually executed and returned the grouped rows.
        let finished = events.iter().find_map(|e| match e {
            AgentEvent::ToolFinished { name, success, result, ui } if name == "run_sql" => {
                Some((*success, result.clone(), ui.clone()))
            }
            _ => None,
        });
        let (success, result, ui) = finished.expect("expected run_sql to finish");
        assert!(success);
        assert!(result.contains("USA"));
        assert_eq!(ui.unwrap()["rows"].as_array().unwrap().len(), 2);

        // Loop ended with the model's final text.
        assert!(matches!(events.last(), Some(AgentEvent::FinalText(t)) if t.contains("USA")));

        let _ = std::fs::remove_file(&path);
    }
}
