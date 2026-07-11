//! Python bindings for opendbpylot (via PyO3 / maturin).
//!
//! ```python
//! import opendbpylot
//!
//! # Uses the configuration written by `dbpylot init` (~/.opendbpylot/).
//! bot = opendbpylot.OpenDbPylot()
//! result = bot.ask("how many orders per country?")
//! print(result["sql"])
//! for row in result["rows"]:
//!     print(row)
//! ```

use std::sync::Arc;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use opendbpylot::app;
use opendbpylot::conversation::MemoryConversationStore;
use opendbpylot::opendbpylot::OpenDbPylot as CoreEngine;
use opendbpylot::secret::{EncryptedFileSecretStore, FileSecretStore, SecretStore};
use opendbpylot::settings::Settings;

/// Build the configured engine from the shared `~/.opendbpylot` config + vault —
/// the exact same setup `dbpylot init` writes and the CLI/web app read.
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

fn shell_out(subcommand: &str) -> PyResult<()> {
    let status = std::process::Command::new("dbpylot")
        .arg(subcommand)
        .status()
        .map_err(|e| {
            PyRuntimeError::new_err(format!(
                "Failed to launch `dbpylot {subcommand}`: {e}. Install the CLI with \
                 `cargo install opendbpylot` and make sure it is on your PATH."
            ))
        })?;
    if !status.success() {
        return Err(PyRuntimeError::new_err(format!("`dbpylot {subcommand}` exited with an error")));
    }
    Ok(())
}

/// The opendbpylot engine: ask natural-language questions and train the model.
#[pyclass]
struct OpenDbPylot {
    inner: Arc<CoreEngine>,
    rt: Arc<tokio::runtime::Runtime>,
}

#[pymethods]
impl OpenDbPylot {
    /// Load the engine from your saved configuration (run `OpenDbPylot.init()` or
    /// `dbpylot init` first to configure an LLM provider and a database).
    #[new]
    fn new() -> PyResult<Self> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PyRuntimeError::new_err(format!("failed to start async runtime: {e}")))?;
        let engine = load_engine()
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))?
            .ok_or_else(|| {
                PyRuntimeError::new_err(
                    "opendbpylot isn't configured yet — run `dbpylot init` (or OpenDbPylot.init()) \
                     to choose an LLM provider and connect a database.",
                )
            })?;
        Ok(Self { inner: Arc::new(engine), rt: Arc::new(rt) })
    }

    /// Ask a natural-language question. Returns a dict with `sql`, `columns`,
    /// `rows`, `repairs_used`, and (if enabled) `answer`.
    fn ask(&self, question: &str) -> PyResult<Py<PyDict>> {
        let res = self
            .rt
            .block_on(self.inner.ask(question))
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))?;
        Python::with_gil(|py| {
            let d = PyDict::new_bound(py);
            d.set_item("sql", res.sql)?;
            d.set_item("repairs_used", res.repairs_used)?;
            match res.result {
                Some(r) => {
                    d.set_item("columns", r.columns)?;
                    d.set_item("rows", r.rows)?;
                }
                None => {
                    d.set_item("columns", Vec::<String>::new())?;
                    d.set_item("rows", Vec::<Vec<String>>::new())?;
                }
            }
            if let Some(a) = res.answer {
                d.set_item("answer", a)?;
            }
            Ok(d.unbind())
        })
    }

    /// Teach the model a table definition (DDL).
    fn train_ddl(&self, ddl: &str) -> PyResult<()> {
        self.rt
            .block_on(self.inner.train_ddl(ddl))
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
    }

    /// Teach the model a business note / documentation string.
    fn train_documentation(&self, doc: &str) -> PyResult<()> {
        self.rt
            .block_on(self.inner.train_documentation(doc))
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
    }

    /// Teach the model a question → SQL example.
    fn train_question_sql(&self, question: &str, sql: &str) -> PyResult<()> {
        self.rt
            .block_on(self.inner.train_question_sql(question, sql))
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
    }

    /// Run the interactive setup wizard (delegates to the `dbpylot` CLI).
    #[staticmethod]
    fn init() -> PyResult<()> {
        shell_out("init")
    }

    /// Launch the embedded web UI (frontend + backend) and open a browser.
    /// Runs in-process until interrupted — no CLI binary required. Configure your
    /// LLM provider and database from the Settings panel.
    #[staticmethod]
    fn serve() -> PyResult<()> {
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| PyRuntimeError::new_err(format!("failed to start async runtime: {e}")))?;
        rt.block_on(opendbpylot::server::run(true))
            .map_err(|e| PyRuntimeError::new_err(format!("{e:#}")))
    }

    /// Test the configured LLM + database connections (delegates to `dbpylot`).
    #[staticmethod]
    fn doctor() -> PyResult<()> {
        shell_out("doctor")
    }
}

/// Run the full `dbpylot` CLI in-process (the `dbpylot` console script calls
/// this). `args[0]` is the program name. Returns the process exit code.
#[pyfunction]
fn run_cli(args: Vec<String>) -> i32 {
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

/// opendbpylot — natural-language → SQL via Retrieval-Augmented Generation.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<OpenDbPylot>()?;
    m.add_function(wrap_pyfunction!(run_cli, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
