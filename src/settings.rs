//! User-editable settings (non-secret). The API key lives in the [`crate::secret`]
//! vault, not here.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    /// "openai" | "anthropic" | "ollama" | "mock"
    pub provider: String,
    /// Model id; empty means "use the provider default".
    pub model: String,
    /// "sqlite" | "postgres" | "mysql"
    pub db_kind: String,
    /// Path to the SQLite file (used when db_kind = "sqlite").
    pub db_path: String,
    /// Full connection URL for remote databases (used when db_kind = "postgres" | "mysql").
    /// Example: "postgresql://user:pass@host:5432/mydb" or "mysql://user:pass@host:3306/mydb".
    ///
    /// Contains the database password, so it is NEVER persisted to settings.json
    /// (`#[serde(skip)]`) — it lives in the encrypted secret vault under the key
    /// `db_connection_string` and is loaded into this field at runtime.
    #[serde(skip)]
    pub db_connection_string: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "mock".into(),
            model: String::new(),
            db_kind: "sqlite".into(),
            db_path: "demo.db".into(),
            db_connection_string: String::new(),
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
