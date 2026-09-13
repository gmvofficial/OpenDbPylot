//! Node.js bindings for opendbpylot (via NAPI-RS).
//!
//! ```typescript
//! import { OpenDbPylot } from 'opendbpylot';
//!
//! const bot = new OpenDbPylot();                 // uses config from `dbpylot init`
//! const result = bot.ask('how many orders per country?');
//! console.log(result.sql);
//! for (const row of result.rows) console.log(row);
//! ```

use std::sync::Arc;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use opendbpylot::app;
use opendbpylot::conversation::MemoryConversationStore;
use opendbpylot::opendbpylot::OpenDbPylot as CoreEngine;
use opendbpylot::secret::{EncryptedFileSecretStore, FileSecretStore, SecretStore};
use opendbpylot::settings::Settings;

/// Build the configured engine from the shared `~/.opendbpylot` config + vault —
/// the same setup `dbpylot init` writes and the CLI/web app read.
fn load_engine() -> anyhow::Result<Option<CoreEngine>> {
    let secrets: Arc<dyn SecretStore> = match std::env::var("OPENDBPYLOT_SECRETS").as_deref() {
        Ok("file") => Arc::new(FileSecretStore::new(app::home().join("secrets.json"))?),
        _ => Arc::new(EncryptedFileSecretStore::new(app::home().join("secrets.enc"))?),
    };
    let mut settings = Settings::load(&app::home().join("settings.json"));
    if settings.db_connection_string.is_empty() {
        if let Ok(Some(c)) = secrets.get("db_connection_string") {
            settings.db_connection_string = c;
        }
    }
    let conversations = Arc::new(MemoryConversationStore::new());
    app::build_opendbpylot(&settings, &*secrets, conversations)
}

fn shell_out(subcommand: &str) -> Result<()> {
    let status = std::process::Command::new("dbpylot")
        .arg(subcommand)
        .status()
        .map_err(|e| {
            Error::from_reason(format!(
                "Failed to launch `dbpylot {subcommand}`: {e}. Install the CLI with \
                 `cargo install opendbpylot` and make sure it is on your PATH."
            ))
        })?;
    if !status.success() {
        return Err(Error::from_reason(format!("`dbpylot {subcommand}` exited with an error")));
    }
    Ok(())
}

/// Result of `OpenDbPylot.ask()`.
#[napi(object)]
pub struct AskResult {
    pub sql: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub repairs_used: u32,
    pub answer: Option<String>,
}

/// The opendbpylot engine: ask natural-language questions and train the model.
#[napi(js_name = "OpenDbPylot")]
pub struct OpenDbPylot {
    inner: Arc<CoreEngine>,
    rt: Arc<tokio::runtime::Runtime>,
}

#[napi]
impl OpenDbPylot {
    /// Load the engine from your saved configuration. Run `OpenDbPylot.init()` or
    /// `dbpylot init` first to configure an LLM provider and a database.
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| Error::from_reason(format!("failed to start async runtime: {e}")))?;
        let engine = load_engine()
            .map_err(|e| Error::from_reason(format!("{e:#}")))?
            .ok_or_else(|| {
                Error::from_reason(
                    "opendbpylot isn't configured yet — run `dbpylot init` (or OpenDbPylot.init()) \
                     to choose an LLM provider and connect a database.",
                )
            })?;
        Ok(Self { inner: Arc::new(engine), rt: Arc::new(rt) })
    }

    /// Ask a natural-language question.
    #[napi]
    pub fn ask(&self, question: String) -> Result<AskResult> {
        let res = self
            .rt
            .block_on(self.inner.ask(&question))
            .map_err(|e| Error::from_reason(format!("{e:#}")))?;
        let (columns, rows) = match res.result {
            Some(r) => (r.columns, r.rows),
            None => (Vec::new(), Vec::new()),
        };
        Ok(AskResult {
            sql: res.sql,
            columns,
            rows,
            repairs_used: res.repairs_used as u32,
            answer: res.answer,
        })
    }

    /// Teach the model a table definition (DDL).
    #[napi]
    pub fn train_ddl(&self, ddl: String) -> Result<()> {
        self.rt
            .block_on(self.inner.train_ddl(&ddl))
            .map_err(|e| Error::from_reason(format!("{e:#}")))
    }

    /// Teach the model a business note / documentation string.
    #[napi]
    pub fn train_documentation(&self, doc: String) -> Result<()> {
        self.rt
            .block_on(self.inner.train_documentation(&doc))
            .map_err(|e| Error::from_reason(format!("{e:#}")))
    }

    /// Teach the model a question → SQL example.
    #[napi]
    pub fn train_question_sql(&self, question: String, sql: String) -> Result<()> {
        self.rt
            .block_on(self.inner.train_question_sql(&question, &sql))
            .map_err(|e| Error::from_reason(format!("{e:#}")))
    }

    /// Run the interactive setup wizard (delegates to the `dbpylot` CLI).
    #[napi]
    pub fn init() -> Result<()> {
        shell_out("init")
    }

    /// Launch the embedded web UI (frontend + backend) and open a browser.
    /// Runs in-process until interrupted — no CLI binary required. Configure your
    /// LLM provider and database from the Settings panel.
    ///
    /// `port` binds on 127.0.0.1 and defaults to 8080, matching `dbpylot serve`.
    /// A host embedding the app can pass an ephemeral port so several instances
    /// coexist without colliding.
    #[napi]
    pub fn serve(port: Option<u16>) -> Result<()> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| Error::from_reason(format!("failed to start async runtime: {e}")))?;
        rt.block_on(opendbpylot::server::run(true, port.unwrap_or(8080)))
            .map_err(|e| Error::from_reason(format!("{e:#}")))
    }

    /// Test the configured LLM + database connections (delegates to `dbpylot`).
    #[napi]
    pub fn doctor() -> Result<()> {
        shell_out("doctor")
    }
}

/// Run the full `dbpylot` CLI in-process (the `dbpylot` bin shim calls this).
/// `args[0]` is the program name. Returns the process exit code.
#[napi(js_name = "runCli")]
pub fn run_cli(args: Vec<String>) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("failed to start async runtime: {e}");
            return 1;
        }
    };
    match rt.block_on(opendbpylot::cli::run_with(args)) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}
