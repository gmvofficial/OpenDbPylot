//! User-editable settings (non-secret). The API key lives in the [`crate::secret`]
//! vault, not here.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    /// "openai" | "anthropic" | "ollama" (plus "mock", internal/testing only —
    /// not offered in the UI).
    pub provider: String,
    /// Model id; empty means "use the provider default".
    pub model: String,
    /// "sqlite" | "postgres" | "mysql" | "duckdb"
    pub db_kind: String,
    /// Path to the SQLite/DuckDB file (used when db_kind = "sqlite" | "duckdb").
    /// For DuckDB, ":memory:" opens an in-memory database and queries can read
    /// local files directly (e.g. `SELECT * FROM 'sales.csv'`).
    pub db_path: String,
    /// Full connection URL for remote databases (used when db_kind = "postgres" | "mysql").
    /// Example: "postgresql://user:pass@host:5432/mydb" or "mysql://user:pass@host:3306/mydb".
    ///
    /// Contains the database password, so it is NEVER persisted to settings.json
    /// (`#[serde(skip)]`) — it lives in the encrypted secret vault under the key
    /// `db_connection_string` and is loaded into this field at runtime.
    #[serde(skip)]
    pub db_connection_string: String,
    /// Embedding backend: "auto" (OpenAI when a key exists, else local),
    /// "local" (offline hashed bag-of-words), "openai", or "fastembed"
    /// (real local semantic model; needs the `fastembed` compile feature).
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,
    /// LLM request timeout in seconds. 0 = per-provider default
    /// (120s for hosted APIs, 300s for Ollama).
    #[serde(default)]
    pub llm_timeout_secs: u64,
    /// Max automatic retries for transient LLM failures (timeouts, 429, 5xx).
    #[serde(default = "default_llm_max_retries")]
    pub llm_max_retries: u32,
}

fn default_llm_max_retries() -> u32 {
    3
}

fn default_embedding_provider() -> String {
    "auto".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Setup-first: a fresh install has no API key, so the app boots
            // NOT-connected and opens Settings — the user configures an LLM and
            // database before anything pretends to work.
            provider: "openai".into(),
            model: String::new(),
            db_kind: "sqlite".into(),
            db_path: "demo.db".into(),
            db_connection_string: String::new(),
            embedding_provider: default_embedding_provider(),
            llm_timeout_secs: 0,
            llm_max_retries: default_llm_max_retries(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// The model to use, falling back to a sensible default per provider.
    pub fn effective_model(&self) -> String {
        if !self.model.is_empty() {
            return self.model.clone();
        }
        match self.provider.as_str() {
            "openai" => "gpt-4o-mini",
            "anthropic" => "claude-sonnet-4-5",
            "ollama" => "llama3",
            _ => "",
        }
        .to_string()
    }
}
