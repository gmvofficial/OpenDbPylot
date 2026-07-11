//! Self-serve web server: settings/vault, training, and multi-conversation chat.
//!
//! The whole UI (a Lit web component) is embedded in the binary at compile time,
//! so this one server *is* the frontend + backend — a single self-contained app.
//! [`run`] is invoked by `dbpylot serve` (opening a browser unless `--headless`).
//! Configure the LLM + database from the in-app Settings sidebar — no `.env` needed.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use async_stream::stream;
use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Path, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::RwLock;

use tokio_stream::StreamExt;

use crate::app;
use crate::conversation::{ConversationStore, FileConversationStore};
use crate::core::agent::{Agent, AgentEvent};
use crate::core::tool::ToolContext;
use crate::llm::Message as LlmMessage;
use crate::secret::{EncryptedFileSecretStore, FileSecretStore, SecretStore};
use crate::settings::Settings;
use crate::opendbpylot::OpenDbPylot;

struct AppCore {
    settings: Settings,
    secrets: Arc<dyn SecretStore>,
    conversations: Arc<dyn ConversationStore>,
    /// Legacy single-shot path — used for training, schema import, turn recording.
    opendbpylot: Option<Arc<OpenDbPylot>>,
    /// The 2.0 tool loop that powers the chat endpoints.
    agent: Option<Arc<Agent>>,
}

#[derive(Clone)]
struct AppState {
    core: Arc<RwLock<AppCore>>,
}

type ApiResult = std::result::Result<Json<Value>, (StatusCode, String)>;
fn api_err<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Boot the web app on `127.0.0.1:8080`. When `open_browser` is true, opens the
/// user's default browser at the URL once the listener is up.
pub async fn run(open_browser: bool) -> Result<()> {
    app::init_tracing();
    let mut settings = Settings::load(&app::home().join("settings.json"));

    // Default: AES-256-GCM encrypted file vault — no keychain prompts.
    // Override with OPENDBPYLOT_SECRETS=file for a plain JSON file (CI / debugging).
    let secrets: Arc<dyn SecretStore> = match std::env::var("OPENDBPYLOT_SECRETS").as_deref() {
        Ok("file") => Arc::new(FileSecretStore::new(app::home().join("secrets.json"))?),
        _ => Arc::new(EncryptedFileSecretStore::new(app::home().join("secrets.enc"))?),
    };

    // The DB connection string (with its password) is kept in the vault, not in
    // settings.json — load it into the in-memory settings.
    if settings.db_connection_string.is_empty() {
        if let Ok(Some(c)) = secrets.get("db_connection_string") {
            settings.db_connection_string = c;
        }
    }

    let conversations: Arc<dyn ConversationStore> =
        Arc::new(FileConversationStore::new(app::home().join("conversations.json"))?);

    let core_build = app::build_core(&settings, &*secrets, conversations.clone())?;
    let (opendbpylot, agent) = match core_build {
        Some(c) => (Some(c.opendbpylot), Some(c.agent)),
        None => (None, None),
    };

    // If a database is configured and reachable, learn its schema on first run
    // (no-op if this DB's knowledge base already has a schema). This is the same
    // path a user's own database takes — the demo DB is not special-cased.
    if let Some(v) = &opendbpylot {
        let _ = connect_and_import(v).await;
    }

    let state = AppState {
        core: Arc::new(RwLock::new(AppCore { settings, secrets, conversations, opendbpylot, agent })),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/opendbpylot-components.js", get(components_js))
        .route("/api/providers", get(providers))
        .route("/api/settings", get(get_settings).post(post_settings))
        .route("/api/train", post(train))
        .route("/api/learn_schema", post(learn_schema))
        .route("/api/conversations", get(list_conversations).post(new_conversation))
        .route("/api/conversations/:id", get(get_conversation).delete(delete_conversation))
        .route("/api/opendbpylot/v2/starter", get(starter))
        .route("/api/opendbpylot/v2/chat_sse", post(chat_sse))
        .route("/api/opendbpylot/v2/chat_poll", post(chat_poll))
        .route("/api/opendbpylot/v2/chat_websocket", get(chat_ws))
        .with_state(state);

    let addr = "127.0.0.1:8080";
    let url = format!("http://{addr}");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("could not bind {addr} (is opendbpylot already running?): {e}"))?;
    println!("opendbpylot is running at {url}");
    println!("Open Settings there to choose an LLM and connect your database.");
    if open_browser {
        open_in_browser(&url);
    }
    axum::serve(listener, app).await?;
    Ok(())
}

/// Best-effort: open `url` in the OS default browser. Never fails the server.
fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let cmd = ("open", vec![url]);
    #[cfg(target_os = "windows")]
    let cmd = ("cmd", vec!["/C", "start", url]);
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    let cmd = ("xdg-open", vec![url]);
    if std::process::Command::new(cmd.0).args(&cmd.1).spawn().is_err() {
        eprintln!("(couldn't auto-open a browser — visit {url} manually)");
    }
}

/// Outcome of connecting to the database and (if needed) importing its schema.
struct DbStatus {
    connected: bool,
    tables_imported: usize,
    error: Option<String>,
}

/// Verify the database is reachable and, if this database's knowledge base has no
/// schema yet, import it. Returns what happened so the UI can report it accurately.
async fn connect_and_import(opendbpylot: &OpenDbPylot) -> DbStatus {
    if let Err(e) = opendbpylot.test_connection().await {
        return DbStatus { connected: false, tables_imported: 0, error: Some(e.to_string()) };
    }
    let already_learned = !opendbpylot.list_ddl().await.unwrap_or_default().is_empty();
    if already_learned {
        return DbStatus { connected: true, tables_imported: 0, error: None };
    }
    match opendbpylot.train_from_schema().await {
        Ok(n) => DbStatus { connected: true, tables_imported: n, error: None },
        Err(e) => DbStatus { connected: true, tables_imported: 0, error: Some(e.to_string()) },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Static pages
// ─────────────────────────────────────────────────────────────────────────────

async fn index() -> Html<&'static str> {
    Html(r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>opendbpylot</title>
  <style>* { box-sizing: border-box; margin: 0; padding: 0; } html, body { height: 100%; background: #090b10; } opendbpylot-app { display: flex; height: 100vh; }</style>
  <script type="module" src="/opendbpylot-components.js"></script>
</head>
<body>
  <opendbpylot-app theme="dark"></opendbpylot-app>
</body>
</html>"#)
}

async fn components_js() -> impl IntoResponse {
    // Embedded at compile time so the binary is fully self-contained — the web UI
    // works even when installed via `cargo install` (no source tree at runtime).
    const JS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/frontends/dist/opendbpylot-components.js"
    ));
    ([("content-type", "application/javascript; charset=utf-8")], JS).into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Settings & providers
// ─────────────────────────────────────────────────────────────────────────────

async fn providers() -> Json<Value> {
    Json(json!({
        "providers": [
            { "id": "openai",    "label": "OpenAI",            "needs_key": true,  "default_model": "gpt-4o-mini" },
            { "id": "anthropic", "label": "Anthropic (Claude)", "needs_key": true,  "default_model": "claude-sonnet-4-5" },
            { "id": "ollama",    "label": "Ollama (local, no API key)", "needs_key": false, "default_model": "llama3" }
        ]
    }))
}

fn status_json(core: &AppCore) -> Value {
    // Only Ollama is genuinely keyless; everything else must have a stored key
    // before the app may claim to be set up.
    let key_set = core.settings.provider == "ollama"
        || core.secrets.get(&core.settings.provider).ok().flatten().is_some();
    // Mask password in connection string for the API response.
    let masked_conn = mask_connection_string(&core.settings.db_connection_string);
    json!({
        "provider": core.settings.provider,
        "model": core.settings.effective_model(),
        "db_kind": core.settings.db_kind,
        "db_path": core.settings.db_path,
        "db_connection_string": masked_conn,
        "key_set": key_set,
        "ready": core.opendbpylot.is_some(),
        // Compile-time features the UI should adapt to (e.g. only offer DuckDB
        // when this build can actually connect to it).
        "duckdb_available": cfg!(feature = "duckdb"),
    })
}

/// Replace the password in a connection URL with "***" for safe display.
/// e.g. "postgres://user:secret@host/db" → "postgres://user:***@host/db"
fn mask_connection_string(url: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(://[^:/@]+:)([^@]+)(@)").unwrap());
    RE.replace(url, "${1}***${3}").to_string()
}

async fn get_settings(State(state): State<AppState>) -> Json<Value> {
    Json(status_json(&*state.core.read().await))
}

#[derive(Deserialize)]
struct SettingsIn {
    provider: String,
    model: Option<String>,
    db_kind: Option<String>,
    db_path: Option<String>,
    db_connection_string: Option<String>,
    api_key: Option<String>,
}

async fn post_settings(State(state): State<AppState>, Json(inp): Json<SettingsIn>) -> ApiResult {
    let mut core = state.core.write().await;

    core.settings.provider = inp.provider;
    if let Some(m) = inp.model {
        core.settings.model = m;
    }
    if let Some(k) = inp.db_kind {
        if !k.is_empty() { core.settings.db_kind = k; }
    }
    if let Some(p) = inp.db_path {
        if !p.is_empty() { core.settings.db_path = p; }
    }
    if let Some(c) = inp.db_connection_string {
        if !c.is_empty() {
            // Store the connection string (contains a password) in the encrypted
            // vault, and keep it in memory for the immediate rebuild below.
            core.secrets.set("db_connection_string", &c).map_err(api_err)?;
            core.settings.db_connection_string = c;
        }
    }
    if let Some(k) = inp.api_key {
        if !k.is_empty() {
            let provider = core.settings.provider.clone();
            core.secrets.set(&provider, &k).map_err(api_err)?;
        }
    }

    core.settings.save(&app::home().join("settings.json")).map_err(api_err)?;

    let built = app::build_core(&core.settings, &*core.secrets, core.conversations.clone())
        .map_err(api_err)?;
    match built {
        Some(c) => {
            core.opendbpylot = Some(c.opendbpylot);
            core.agent = Some(c.agent);
        }
        None => {
            core.opendbpylot = None;
            core.agent = None;
        }
    }

    // Test the connection and import the schema for this database (so the user
    // doesn't have to do it as a separate step). Report what happened.
    let db = match &core.opendbpylot {
        Some(v) => connect_and_import(v).await,
        None => DbStatus { connected: false, tables_imported: 0, error: None },
    };

    let mut status = status_json(&core);
    status["db_connected"] = json!(db.connected);
    status["tables_imported"] = json!(db.tables_imported);
    if let Some(e) = db.error {
        status["db_error"] = json!(e);
    }
    Ok(Json(status))
}

// ─────────────────────────────────────────────────────────────────────────────
// Training
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TrainIn {
    kind: String,
    text: Option<String>,
    question: Option<String>,
    sql: Option<String>,
}

async fn train(State(state): State<AppState>, Json(inp): Json<TrainIn>) -> ApiResult {
    let opendbpylot = current_opendbpylot(&state).await.ok_or((StatusCode::BAD_REQUEST, "not configured".into()))?;
    match inp.kind.as_str() {
        "ddl" => opendbpylot.train_ddl(&inp.text.unwrap_or_default()).await.map_err(api_err)?,
        "doc" => opendbpylot.train_documentation(&inp.text.unwrap_or_default()).await.map_err(api_err)?,
        "sql" => opendbpylot
            .train_question_sql(&inp.question.unwrap_or_default(), &inp.sql.unwrap_or_default())
            .await
            .map_err(api_err)?,
        other => return Err((StatusCode::BAD_REQUEST, format!("unknown kind: {other}"))),
    }
    Ok(Json(json!({ "ok": true })))
}

async fn learn_schema(State(state): State<AppState>) -> ApiResult {
    let opendbpylot = current_opendbpylot(&state).await.ok_or((StatusCode::BAD_REQUEST, "not configured".into()))?;
    let n = opendbpylot.train_from_schema().await.map_err(api_err)?;
    Ok(Json(json!({ "ok": true, "tables_learned": n })))
}

// ─────────────────────────────────────────────────────────────────────────────
// Conversations
// ─────────────────────────────────────────────────────────────────────────────

async fn list_conversations(State(state): State<AppState>) -> Json<Value> {
    let convs = state.core.read().await.conversations.clone();
    let list = convs.list().await.unwrap_or_default();
    Json(json!({ "conversations": list }))
}

async fn new_conversation() -> Json<Value> {
    Json(json!({ "id": format!("conv-{}", now_ms()) }))
}

async fn get_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    let convs = state.core.read().await.conversations.clone();
    let turns = convs.turns(&id).await.unwrap_or_default();
    let messages: Vec<Value> = turns
        .into_iter()
        .map(|t| json!({ "question": t.question, "sql": t.sql }))
        .collect();
    Json(json!({ "id": id, "messages": messages }))
}

async fn delete_conversation(State(state): State<AppState>, Path(id): Path<String>) -> Json<Value> {
    let convs = state.core.read().await.conversations.clone();
    match convs.delete(&id).await {
        Ok(()) => Json(json!({ "ok": true })),
        Err(e) => Json(json!({ "ok": false, "error": e.to_string() })),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chat (rich {rich, simple} chunk protocol over SSE / poll / WebSocket)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatRequest {
    message: String,
    conversation_id: Option<String>,
    request_id: Option<String>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

fn rich_lc(id: &str, kind: &str, lifecycle: &str, data: Value) -> Value {
    json!({ "id": id, "type": kind, "lifecycle": lifecycle, "data": data,
            "children": [], "visible": true, "interactive": false })
}
fn rich(id: &str, kind: &str, data: Value) -> Value {
    rich_lc(id, kind, "create", data)
}
fn payload(conv: &str, req: &str, rich: Value) -> Value {
    json!({ "rich": rich, "conversation_id": conv, "request_id": req, "timestamp": now_ms() })
}

async fn current_opendbpylot(state: &AppState) -> Option<Arc<OpenDbPylot>> {
    state.core.read().await.opendbpylot.clone()
}

async fn current_agent(state: &AppState) -> Option<Arc<Agent>> {
    state.core.read().await.agent.clone()
}

/// Load recent conversation turns as prior user/assistant messages so the agent
/// has context for follow-up questions.
async fn load_history(state: &AppState, conv: &str) -> Vec<LlmMessage> {
    let convs = state.core.read().await.conversations.clone();
    let recent = convs.recent(conv, 5).await.unwrap_or_default();
    let mut msgs = Vec::with_capacity(recent.len() * 2);
    for t in recent {
        msgs.push(LlmMessage::user(t.question));
        msgs.push(LlmMessage::assistant(t.sql));
    }
    msgs
}

fn ids(req: &ChatRequest) -> (String, String) {
    (
        req.conversation_id.clone().unwrap_or_else(|| format!("conv-{}", now_ms())),
        req.request_id.clone().unwrap_or_else(|| format!("req-{}", now_ms())),
    )
}

/// Drive the agent loop and translate its `AgentEvent`s into the frontend's rich
/// chunk protocol — the SAME wire format the old fixed pipeline used, so the
/// frontend needs no changes. The difference is the steps are now driven by the
/// model's real tool calls, not a hardcoded script.
fn spawn_producer(
    agent: Arc<Agent>,
    opendbpylot: Arc<OpenDbPylot>,
    conv: String,
    request_id: String,
    question: String,
    history: Vec<LlmMessage>,
) -> tokio::sync::mpsc::Receiver<Value> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Value>(32);
    tokio::spawn(async move {
        // Kick off the activity panel.
        let _ = tx.send(payload(&conv, &request_id, rich("status", "status_bar_update",
            json!({"status":"working","message":"Analyzing question…"})))).await;
        let _ = tx.send(payload(&conv, &request_id, rich("prog", "progress_bar",
            json!({"label":"Analyzing question…","value":10})))).await;

        let ctx = ToolContext { conversation_id: conv.clone(), request_id: request_id.clone() };
        let stream = agent.run(ctx, history, question.clone());
        tokio::pin!(stream);

        let mut last_sql: Option<String> = None;
        let mut ran_ok = false;
        let mut seq = 0u64;

        while let Some(ev) = stream.next().await {
            seq += 1;
            match ev {
                AgentEvent::ToolStarted { name, args } => {
                    if name == "run_sql" {
                        if let Some(sql) = args.get("sql").and_then(|v| v.as_str()) {
                            last_sql = Some(sql.to_string());
                            let _ = tx.send(payload(&conv, &request_id,
                                rich(&format!("sql-{request_id}-{seq}"), "text",
                                    json!({"text": sql, "language":"sql", "title":"Generated SQL"})))).await;
                        }
                        let _ = tx.send(payload(&conv, &request_id, rich("status", "status_bar_update",
                            json!({"status":"working","message":"Running query…"})))).await;
                        let _ = tx.send(payload(&conv, &request_id, rich_lc("prog", "progress_bar", "update",
                            json!({"label":"Running query…","value":60})))).await;
                    } else if name == "visualize_data" {
                        let _ = tx.send(payload(&conv, &request_id, rich("status", "status_bar_update",
                            json!({"status":"working","message":"Creating chart…"})))).await;
                        let _ = tx.send(payload(&conv, &request_id, rich_lc("prog", "progress_bar", "update",
                            json!({"label":"Creating chart…","value":80})))).await;
                    } else {
                        let _ = tx.send(payload(&conv, &request_id, rich("status", "status_bar_update",
                            json!({"status":"working","message": format!("Running {name}…")})))).await;
                    }
                }
                AgentEvent::ToolFinished { name, success, result, ui } => {
                    if !success {
                        // Only surface real query errors. A failed visualize_data
                        // (e.g. "data isn't suitable for a chart") is an internal
                        // hint for the model, not a user-facing error — swallow it.
                        if name == "run_sql" {
                            let _ = tx.send(payload(&conv, &request_id,
                                rich(&format!("err-{request_id}-{seq}"), "notification",
                                    json!({"level":"error","message": result})))).await;
                        }
                    } else if let Some(ui) = ui {
                        match ui.get("kind").and_then(|v| v.as_str()) {
                            Some("dataframe") => {
                                let _ = tx.send(payload(&conv, &request_id,
                                    rich(&format!("df-{request_id}-{seq}"), "dataframe", json!({
                                        "columns": ui.get("columns").cloned().unwrap_or_else(|| json!([])),
                                        "rows": ui.get("rows").cloned().unwrap_or_else(|| json!([])),
                                        "title": ui.get("title").cloned().unwrap_or_else(|| json!("Result")),
                                        "description": ui.get("description").cloned().unwrap_or_else(|| json!("")),
                                    })))).await;
                                if name == "run_sql" {
                                    ran_ok = true;
                                }
                            }
                            Some("chart") => {
                                let _ = tx.send(payload(&conv, &request_id,
                                    rich(&format!("chart-{request_id}-{seq}"), "chart",
                                        json!({"spec": ui.get("spec").cloned().unwrap_or_else(|| json!({}))})))).await;
                            }
                            _ => {}
                        }
                    } else if name == "run_sql" {
                        ran_ok = true; // e.g. a successful statement with no rows
                    }
                }
                AgentEvent::FinalText(text) => {
                    if !text.trim().is_empty() {
                        let _ = tx.send(payload(&conv, &request_id,
                            rich(&format!("ans-{request_id}-{seq}"), "text",
                                json!({"text": text, "title":"Answer"})))).await;
                    }
                }
                AgentEvent::Notice(msg) => {
                    let _ = tx.send(payload(&conv, &request_id,
                        rich(&format!("note-{request_id}-{seq}"), "notification",
                            json!({"level":"warning","message": msg})))).await;
                }
            }
        }

        // Persist the successful turn (enables follow-ups + auto-trains the KB).
        if ran_ok {
            if let Some(sql) = last_sql {
                opendbpylot.record_turn(&conv, &question, &sql).await;
            }
        }

        let _ = tx.send(payload(&conv, &request_id, rich_lc("prog", "progress_bar", "update",
            json!({"label":"Done","value":100})))).await;
        let _ = tx.send(payload(&conv, &request_id, rich("status", "status_bar_update",
            json!({"status":"idle","message":"Ready"})))).await;
        let _ = tx.send(payload(&conv, &request_id, rich("input", "chat_input_update",
            json!({"disabled": false})))).await;
    });
    rx
}

fn not_configured_chunk(conv: &str, req: &str) -> Value {
    payload(conv, req, rich("nc", "notification",
        json!({"level":"warning","message":"Not configured yet — open Settings to choose an LLM and add your API key."})))
}

/// Fetch the agent + opendbpylot and load conversation history for a chat request.
/// Returns `None` when the app isn't configured.
async fn prepare_turn(
    state: &AppState,
    conv: &str,
    message: &str,
) -> Option<(Arc<Agent>, Arc<OpenDbPylot>, Vec<LlmMessage>)> {
    let agent = current_agent(state).await?;
    let opendbpylot = current_opendbpylot(state).await?;
    let convs = state.core.read().await.conversations.clone();
    let _ = convs.ensure(conv, message).await;
    let history = load_history(state, conv).await;
    Some((agent, opendbpylot, history))
}

async fn chat_sse(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> impl IntoResponse {
    let (conv, request_id) = ids(&req);
    let prepared = prepare_turn(&state, &conv, &req.message).await;
    let message = req.message;
    let stream = stream! {
        match prepared {
            Some((agent, opendbpylot, history)) => {
                let mut rx = spawn_producer(agent, opendbpylot, conv.clone(), request_id.clone(), message, history);
                while let Some(msg) = rx.recv().await {
                    yield Ok::<Event, Infallible>(Event::default().data(msg.to_string()));
                }
            }
            None => {
                yield Ok::<Event, Infallible>(Event::default().data(not_configured_chunk(&conv, &request_id).to_string()));
            }
        }
        yield Ok::<Event, Infallible>(Event::default().data("[DONE]"));
    };
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn chat_poll(State(state): State<AppState>, Json(req): Json<ChatRequest>) -> Json<Value> {
    let (conv, request_id) = ids(&req);
    let mut chunks = Vec::new();
    match prepare_turn(&state, &conv, &req.message).await {
        Some((agent, opendbpylot, history)) => {
            let mut rx = spawn_producer(agent, opendbpylot, conv.clone(), request_id.clone(), req.message, history);
            while let Some(msg) = rx.recv().await {
                chunks.push(msg);
            }
        }
        None => chunks.push(not_configured_chunk(&conv, &request_id)),
    }
    Json(json!({ "chunks": chunks, "conversation_id": conv, "request_id": request_id, "total_chunks": chunks.len() }))
}

async fn chat_ws(State(state): State<AppState>, ws: WebSocketUpgrade) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(mut socket: WebSocket, state: AppState) {
    while let Some(Ok(msg)) = socket.recv().await {
        if let Message::Text(txt) = msg {
            let req: ChatRequest = match serde_json::from_str(&txt) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let (conv, request_id) = ids(&req);
            match prepare_turn(&state, &conv, &req.message).await {
                Some((agent, opendbpylot, history)) => {
                    let mut rx = spawn_producer(agent, opendbpylot, conv.clone(), request_id.clone(), req.message, history);
                    while let Some(m) = rx.recv().await {
                        if socket.send(Message::Text(m.to_string())).await.is_err() {
                            return;
                        }
                    }
                }
                None => {
                    let _ = socket.send(Message::Text(not_configured_chunk(&conv, &request_id).to_string())).await;
                }
            }
            let done = json!({"rich": {"type":"completion","data":{}}, "conversation_id": conv, "request_id": request_id});
            let _ = socket.send(Message::Text(done.to_string())).await;
        }
    }
}

async fn starter() -> Json<Value> {
    Json(json!({ "suggestions": [
        "How many users are there per country?",
        "What are the names of users from the USA?",
        "How many users in total?",
        "List users created after 2024-06-01"
    ]}))
}
