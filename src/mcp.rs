//! MCP (Model Context Protocol) stdio server: `dbpylot mcp`.
//!
//! Serves the engine's capabilities as MCP tools to any agent host — OpenPylot,
//! Claude Desktop, Claude Code, or the MCP Inspector. JSON-RPC 2.0, one JSON
//! object per line, over stdin/stdout.
//!
//! Protocol invariants (do not break these):
//! - stdout carries *only* JSON-RPC responses — logs go to stderr
//!   (see `app::init_tracing_stderr`), and nothing here may `println!`.
//! - Every incoming message that carries an `id` gets exactly one response
//!   line; messages without an `id` (true notifications) get silence. Some
//!   hosts (OpenPylot) send `notifications/initialized` *with* an id and then
//!   block waiting for a reply, while spec-strict hosts send it without one —
//!   this rule keeps both working.
//! - An unconfigured engine never kills the server: `initialize` and
//!   `tools/list` always work, and `tools/call` retries the (purely local)
//!   engine construction so `dbpylot init` can be run without a restart.

use std::sync::Arc;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::opendbpylot::OpenDbPylot;
use crate::sqlrunner::QueryResult;

/// Rows are capped in tool results so a big table can't blow up the host
/// agent's context window. The full count is reported in `row_count`.
const MAX_RESULT_ROWS: usize = 200;

/// MCP protocol revision we speak (also echoed back from the client if it
/// proposes one — we accept any and let the client decide compatibility).
const PROTOCOL_VERSION: &str = "2024-11-05";

struct McpState {
    /// `None` = not configured yet. Retried lazily on each `tools/call`
    /// (construction is local file I/O only — no network).
    engine: Mutex<Option<Arc<OpenDbPylot>>>,
    /// Whether `engine()` may retry `build_configured()` when the slot is
    /// empty. True in the real server; false in tests, which must not read
    /// the developer's actual `~/.opendbpylot` configuration.
    lazy_rebuild: bool,
}

impl McpState {
    fn new(engine: Option<OpenDbPylot>) -> Self {
        Self { engine: Mutex::new(engine.map(Arc::new)), lazy_rebuild: true }
    }

    /// The configured engine, building it now if setup happened after startup.
    async fn engine(&self) -> Option<Arc<OpenDbPylot>> {
        let mut slot = self.engine.lock().await;
        if slot.is_none() && self.lazy_rebuild {
            if let Ok(Some(bot)) = crate::cli::build_configured() {
                *slot = Some(Arc::new(bot));
            }
        }
        slot.clone()
    }
}

/// Entry point for `dbpylot mcp`: serve MCP over stdin/stdout until EOF.
pub async fn serve_stdio() -> Result<()> {
    // Tolerate an unconfigured or broken setup at startup; tools report it.
    let state = McpState::new(crate::cli::build_configured().unwrap_or(None));

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle_line(&state, &line).await {
            stdout.write_all(response.to_string().as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    // EOF: the host closed the pipe — clean shutdown.
    Ok(())
}

/// Handle one incoming JSON-RPC line. Returns `Some(response)` iff the message
/// carries an `id` (or is unparseable, which gets a parse error).
async fn handle_line(state: &McpState, line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(rpc_error(Value::Null, -32700, "Parse error")),
    };

    // Keep the id as a raw Value: clients may use numbers or strings.
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    let Some(id) = id else {
        // A true notification: nothing may be written back.
        return None;
    };

    let result = match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION);
            json!({
                "protocolVersion": client_version,
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "opendbpylot",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            })
        }
        // Some hosts send the initialized notification with an id and wait for
        // a reply (see module docs); an empty result satisfies them.
        "notifications/initialized" | "initialized" | "ping" => json!({}),
        "tools/list" => json!({ "tools": tool_definitions() }),
        "tools/call" => call_tool(state, &params).await,
        _ => return Some(rpc_error(id, -32601, &format!("Method not found: {method}"))),
    };

    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tools
// ─────────────────────────────────────────────────────────────────────────────

/// The tool catalog returned by `tools/list`. These six names and their
/// argument/result shapes are a stable public contract — agent presets and
/// skills in host applications refer to them by name.
fn tool_definitions() -> Vec<Value> {
    let no_args = json!({ "type": "object", "properties": {} });
    vec![
        json!({
            "name": "ask_database",
            "description": "Ask a natural-language question about the connected database. \
                Generates SQL via retrieval-augmented generation, executes it read-only, \
                and self-repairs failed queries. Returns the SQL, the rows, and optionally \
                a short answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": { "type": "string", "description": "The question, in plain English" }
                },
                "required": ["question"]
            }
        }),
        json!({
            "name": "run_sql",
            "description": "Run a read-only SQL query (SELECT/WITH only) directly against \
                the connected database. Write statements are rejected.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "A single read-only SQL statement" }
                },
                "required": ["sql"]
            }
        }),
        json!({
            "name": "list_schema",
            "description": "List everything the engine knows about the database: learned DDL, \
                documentation notes, and example questions it was trained on.",
            "inputSchema": no_args,
        }),
        json!({
            "name": "refresh_schema",
            "description": "Re-introspect the live database schema and reload it into the \
                knowledge base. Use after the database structure changes.",
            "inputSchema": no_args,
        }),
        json!({
            "name": "train",
            "description": "Teach the engine: a table DDL, a documentation note, or a \
                question→SQL example pair. Improves future SQL generation.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {
                        "type": "string",
                        "enum": ["ddl", "documentation", "question_sql"],
                        "description": "What kind of training material this is"
                    },
                    "content": { "type": "string", "description": "The DDL or documentation text (for kind=ddl/documentation)" },
                    "question": { "type": "string", "description": "For kind=question_sql" },
                    "sql": { "type": "string", "description": "For kind=question_sql" }
                },
                "required": ["kind"]
            }
        }),
        json!({
            "name": "health",
            "description": "Check whether opendbpylot is configured and the database is \
                reachable. Call this first if other tools fail.",
            "inputSchema": no_args,
        }),
    ]
}

/// Wrap a JSON payload as an MCP tool result (a single text content block).
fn text_result(payload: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": payload.to_string() }],
        "isError": is_error,
    })
}

fn error_result(message: impl Into<String>) -> Value {
    text_result(json!({ "error": scrub_secrets(&message.into()) }), true)
}

/// Redact credentials from any connection-URL-like substring so a database
/// driver error can never leak the password to the MCP host — which may
/// forward tool output to a cloud LLM. Turns `scheme://user:pass@host` into
/// `scheme://user:***@host`.
fn scrub_secrets(text: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"([a-zA-Z][a-zA-Z0-9+.\-]*://[^:/@\s]+):[^@/\s]+@").unwrap()
    });
    RE.replace_all(text, "$1:***@").into_owned()
}

/// A query result as JSON, capped at [`MAX_RESULT_ROWS`] rows.
fn table_json(result: &QueryResult) -> Value {
    let total = result.rows.len();
    let truncated = total > MAX_RESULT_ROWS;
    let rows: &[Vec<String>] = if truncated { &result.rows[..MAX_RESULT_ROWS] } else { &result.rows };
    json!({
        "columns": result.columns,
        "rows": rows,
        "row_count": total,
        "truncated": truncated,
    })
}

/// Dispatch a `tools/call` request. Tool-level failures are MCP results with
/// `isError: true` (never JSON-RPC errors) so the host's LLM can read them.
async fn call_tool(state: &McpState, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    // `run_sql` validates before touching the engine so the read-only gate is
    // enforced even while unconfigured (and testable that way).
    if name == "run_sql" {
        let sql = args.get("sql").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if sql.is_empty() {
            return error_result("Missing required argument: sql");
        }
        if !crate::sql::is_read_only(&sql) {
            return error_result(
                "Blocked: only read-only queries (SELECT / WITH) are allowed. \
                 This tool cannot modify or delete data.",
            );
        }
        let Some(bot) = state.engine().await else { return not_configured() };
        return match bot.run_sql(&sql).await {
            Ok(result) => text_result(table_json(&result), false),
            Err(e) => error_result(format!("SQL error: {e}")),
        };
    }

    // `health` reports rather than errors, even when unconfigured.
    if name == "health" {
        return match state.engine().await {
            None => text_result(
                json!({ "configured": false, "connected": false,
                        "error": "not configured — run `dbpylot init`" }),
                false,
            ),
            Some(bot) => match bot.test_connection().await {
                Ok(()) => text_result(json!({ "configured": true, "connected": true, "error": null }), false),
                Err(e) => text_result(
                    json!({ "configured": true, "connected": false, "error": scrub_secrets(&e.to_string()) }),
                    false,
                ),
            },
        };
    }

    let Some(bot) = state.engine().await else { return not_configured() };

    match name {
        "ask_database" => {
            let Some(question) = args.get("question").and_then(Value::as_str).filter(|q| !q.trim().is_empty())
            else {
                return error_result("Missing required argument: question");
            };
            match bot.ask(question).await {
                Ok(ask) => {
                    let mut payload = json!({
                        "sql": ask.sql,
                        "repairs_used": ask.repairs_used,
                        "answer": ask.answer,
                        "executed": ask.result.is_some(),
                    });
                    match &ask.result {
                        Some(result) => {
                            let table = table_json(result);
                            for (k, v) in table.as_object().unwrap() {
                                payload[k.as_str()] = v.clone();
                            }
                        }
                        None => {
                            payload["note"] =
                                json!("not run — the generated SQL was not a read query");
                        }
                    }
                    text_result(payload, false)
                }
                Err(e) => error_result(format!("ask failed: {e}")),
            }
        }
        "list_schema" => {
            let ddl = bot.list_ddl().await.unwrap_or_default();
            let documentation = bot.list_documentation().await.unwrap_or_default();
            let example_questions: Vec<String> = bot
                .list_question_sql()
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|qs| qs.question)
                .collect();
            text_result(
                json!({
                    "ddl": ddl,
                    "documentation": documentation,
                    "example_questions": example_questions,
                }),
                false,
            )
        }
        "refresh_schema" => match bot.train_from_schema().await {
            Ok(n) => text_result(json!({ "tables_learned": n }), false),
            Err(e) => error_result(format!("schema refresh failed: {e}")),
        },
        "train" => {
            let kind = args.get("kind").and_then(Value::as_str).unwrap_or("");
            let content = args.get("content").and_then(Value::as_str).unwrap_or("");
            let outcome = match kind {
                "ddl" if !content.trim().is_empty() => bot.train_ddl(content).await,
                "documentation" if !content.trim().is_empty() => bot.train_documentation(content).await,
                "question_sql" => {
                    let question = args.get("question").and_then(Value::as_str).unwrap_or("");
                    let sql = args.get("sql").and_then(Value::as_str).unwrap_or("");
                    if question.trim().is_empty() || sql.trim().is_empty() {
                        return error_result(
                            "kind=question_sql requires both `question` and `sql` arguments",
                        );
                    }
                    bot.train_question_sql(question, sql).await
                }
                "ddl" | "documentation" => {
                    return error_result(format!("kind={kind} requires a non-empty `content` argument"))
                }
                _ => {
                    return error_result(
                        "Invalid `kind`: expected one of ddl, documentation, question_sql",
                    )
                }
            };
            match outcome {
                Ok(()) => text_result(json!({ "trained": kind }), false),
                Err(e) => error_result(format!("training failed: {e}")),
            }
        }
        "" => error_result("Missing tool name"),
        other => error_result(format!("Unknown tool: {other}")),
    }
}

fn not_configured() -> Value {
    error_result(
        "opendbpylot is not configured. Run `dbpylot init` in a terminal to choose \
         an LLM provider and connect a database, then retry.",
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::local::LocalEmbedding;
    use crate::llm::mock::MockLlm;
    use crate::sqlrunner::sqlite::SqliteRunner;
    use crate::vectorstore::memory::MemoryVectorStore;

    fn unconfigured() -> McpState {
        let mut state = McpState::new(None);
        state.lazy_rebuild = false; // never read the developer's real config
        state
    }

    /// An engine over an in-memory SQLite DB with a mock LLM — no network.
    fn configured() -> McpState {
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(Arc::new(MockLlm::with_default_sql()), store)
            .with_runner(Arc::new(SqliteRunner::new(":memory:".to_string())));
        McpState::new(Some(bot))
    }

    async fn call(state: &McpState, line: &str) -> Option<Value> {
        handle_line(state, line).await
    }

    fn tool_call_line(id: u64, tool: &str, args: Value) -> String {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": tool, "arguments": args }
        })
        .to_string()
    }

    /// Parse the JSON payload out of a tool result's text content block.
    fn tool_payload(resp: &Value) -> (Value, bool) {
        let result = &resp["result"];
        let text = result["content"][0]["text"].as_str().unwrap();
        let is_error = result["isError"].as_bool().unwrap();
        (serde_json::from_str(text).unwrap(), is_error)
    }

    #[tokio::test]
    async fn initialize_reports_server_info() {
        let resp = call(
            &unconfigured(),
            &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2024-11-05","capabilities":{},
                "clientInfo":{"name":"test","version":"0"}}})
            .to_string(),
        )
        .await
        .unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "opendbpylot");
    }

    #[tokio::test]
    async fn initialized_with_id_gets_reply_without_id_gets_silence() {
        let state = unconfigured();
        // OpenPylot-style: notification WITH an id — must get a response.
        let with_id = call(
            &state,
            &json!({"jsonrpc":"2.0","id":2,"method":"notifications/initialized"}).to_string(),
        )
        .await;
        assert!(with_id.is_some());
        assert_eq!(with_id.unwrap()["id"], 2);
        // Spec-strict: no id — must stay silent.
        let without_id =
            call(&state, &json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string())
                .await;
        assert!(without_id.is_none());
    }

    #[tokio::test]
    async fn tools_list_exposes_the_six_tools() {
        let resp = call(
            &unconfigured(),
            &json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}).to_string(),
        )
        .await
        .unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            ["ask_database", "run_sql", "list_schema", "refresh_schema", "train", "health"]
        );
        for tool in tools {
            assert!(tool["inputSchema"].is_object(), "tool {} lacks inputSchema", tool["name"]);
        }
    }

    #[test]
    fn scrub_secrets_redacts_connection_passwords() {
        // A driver error that embeds the DSN must not leak the password.
        let leaked = "failed to connect: postgres://admin:s3cret@db.host:5432/shop is unreachable";
        let scrubbed = scrub_secrets(leaked);
        assert!(!scrubbed.contains("s3cret"), "password leaked: {scrubbed}");
        assert!(scrubbed.contains("postgres://admin:***@db.host"));
        // Non-URL text is untouched.
        assert_eq!(scrub_secrets("no such table: users"), "no such table: users");
    }

    #[tokio::test]
    async fn run_sql_blocks_writes_even_when_unconfigured() {
        let resp = call(
            &unconfigured(),
            &tool_call_line(4, "run_sql", json!({"sql": "DROP TABLE users"})),
        )
        .await
        .unwrap();
        let (payload, is_error) = tool_payload(&resp);
        assert!(is_error);
        assert!(payload["error"].as_str().unwrap().contains("read-only"));
    }

    #[tokio::test]
    async fn ask_database_unconfigured_points_to_init() {
        let resp = call(
            &unconfigured(),
            &tool_call_line(5, "ask_database", json!({"question": "how many users?"})),
        )
        .await
        .unwrap();
        let (payload, is_error) = tool_payload(&resp);
        assert!(is_error);
        assert!(payload["error"].as_str().unwrap().contains("dbpylot init"));
    }

    #[tokio::test]
    async fn unknown_method_and_garbage_get_json_rpc_errors() {
        let state = unconfigured();
        let unknown =
            call(&state, &json!({"jsonrpc":"2.0","id":6,"method":"resources/list"}).to_string())
                .await
                .unwrap();
        assert_eq!(unknown["error"]["code"], -32601);

        let garbage = call(&state, "this is not json").await.unwrap();
        assert_eq!(garbage["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn run_sql_returns_rows_against_a_real_engine() {
        let resp = call(
            &configured(),
            &tool_call_line(7, "run_sql", json!({"sql": "SELECT 1 AS one, 2 AS two"})),
        )
        .await
        .unwrap();
        let (payload, is_error) = tool_payload(&resp);
        assert!(!is_error);
        assert_eq!(payload["columns"], json!(["one", "two"]));
        assert_eq!(payload["rows"], json!([["1", "2"]]));
        assert_eq!(payload["row_count"], 1);
        assert_eq!(payload["truncated"], false);
    }

    #[tokio::test]
    async fn train_documentation_round_trips_through_list_schema() {
        let state = configured();
        let trained = call(
            &state,
            &tool_call_line(8, "train", json!({"kind": "documentation",
                "content": "Revenue excludes cancelled orders."})),
        )
        .await
        .unwrap();
        let (payload, is_error) = tool_payload(&trained);
        assert!(!is_error);
        assert_eq!(payload["trained"], "documentation");

        let listed = call(&state, &tool_call_line(9, "list_schema", json!({}))).await.unwrap();
        let (payload, is_error) = tool_payload(&listed);
        assert!(!is_error);
        assert_eq!(payload["documentation"], json!(["Revenue excludes cancelled orders."]));
    }

    #[tokio::test]
    async fn train_validates_arguments() {
        let state = configured();
        let bad_kind =
            call(&state, &tool_call_line(10, "train", json!({"kind": "nonsense"}))).await.unwrap();
        assert!(tool_payload(&bad_kind).1);

        let missing_sql = call(
            &state,
            &tool_call_line(11, "train", json!({"kind": "question_sql", "question": "q"})),
        )
        .await
        .unwrap();
        assert!(tool_payload(&missing_sql).1);
    }
}
