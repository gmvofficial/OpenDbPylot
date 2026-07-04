//! Application wiring: build the runtime objects from saved settings + the secret
//! vault. Used by the server so configuration can change at runtime (not via `.env`).
//!
//! There are two runtime objects, sharing the same LLM / vector store / SQL runner:
//! - `OpenDbPylot`  — the legacy single-shot path (still used for training, schema import,
//!              conversation recording).
//! - `Agent`  — the 2.0 tool loop that powers the chat endpoint.
//! Sharing the same `Arc<dyn VectorStore>` means knowledge trained via `OpenDbPylot` is
//! immediately visible to the `Agent`'s RAG enhancer.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::capabilities::file_system::{FileSystem, MemoryFileSystem};
use crate::conversation::ConversationStore;
use crate::core::agent::Agent;
use crate::core::enhancer::RagEnhancer;
use crate::core::registry::ToolRegistry;
use crate::core::system_prompt::build_sql_system_prompt;
use crate::embedding::{local::LocalEmbedding, openai::OpenAiEmbedding, EmbeddingService};
use crate::llm::{
    anthropic::AnthropicLlm, mock::MockLlm, ollama::OllamaLlm, openai::OpenAiLlm, LlmService,
};
use crate::secret::SecretStore;
use crate::settings::Settings;
use crate::sqlrunner::sqlite::SqliteRunner;
use crate::sqlrunner::SqlRunner;
use crate::tools::memory::{RememberTool, SaveQueryTool};
use crate::tools::run_sql::RunSqlTool;
use crate::tools::visualize_data::VisualizeDataTool;
use crate::opendbpylot::{OpenDbPylot, OpenDbPylotConfig};
use crate::vectorstore::{file::FileVectorStore, VectorStore};

/// A stable identifier for the connected database (kind + target). Used to scope
/// the knowledge base so each database keeps its own learned schema.
fn db_identity(settings: &Settings) -> String {
    match settings.db_kind.as_str() {
        "postgres" | "postgresql" | "mysql" | "mariadb" => {
            format!("{}:{}", settings.db_kind, settings.db_connection_string)
        }
        _ => format!("sqlite:{}", settings.db_path),
    }
}

/// KB filename namespaced by embedder (vector dims must not clash) AND database
/// (so connecting a different DB doesn't inherit the previous one's schema).
fn kb_filename(embedder: &str, settings: &Settings) -> String {
    let mut h = DefaultHasher::new();
    db_identity(settings).hash(&mut h);
    format!("kb_{}_{:016x}.json", embedder, h.finish())
}

/// Map a `db_kind` to the SQL dialect name used in prompts.
fn dialect_for(db_kind: &str) -> &'static str {
    match db_kind {
        "postgres" | "postgresql" => "PostgreSQL",
        "mysql" | "mariadb" => "MySQL",
        _ => "SQLite",
    }
}

/// Build the SQL runner from settings. Returns a trait object that can be swapped
/// between SQLite, PostgreSQL, or MySQL without changing the rest of the app.
pub fn build_runner(settings: &Settings) -> Result<Arc<dyn SqlRunner>> {
    match settings.db_kind.as_str() {
        #[cfg(feature = "remote-db")]
        "postgres" | "postgresql" => {
            use crate::sqlrunner::postgres::PostgresRunner;
            if settings.db_connection_string.is_empty() {
                return Err(anyhow::anyhow!("db_connection_string is required for PostgreSQL"));
            }
            Ok(Arc::new(PostgresRunner::new(settings.db_connection_string.clone())))
        }
        #[cfg(feature = "remote-db")]
        "mysql" | "mariadb" => {
            use crate::sqlrunner::mysql::MySqlRunner;
            if settings.db_connection_string.is_empty() {
                return Err(anyhow::anyhow!("db_connection_string is required for MySQL"));
            }
            Ok(Arc::new(MySqlRunner::new(settings.db_connection_string.clone())))
        }
        // Default: SQLite
        _ => Ok(Arc::new(SqliteRunner::new(settings.db_path.clone()))),
    }
}

/// Per-user data directory (`$OPENDBPYLOT_HOME` or `~/.opendbpylot`), created if needed.
pub fn home() -> PathBuf {
    let dir = std::env::var("OPENDBPYLOT_HOME").map(PathBuf::from).unwrap_or_else(|_| {
        let base = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join(".opendbpylot")
    });
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The shared building blocks both `OpenDbPylot` and `Agent` are assembled from.
struct Components {
    llm: Arc<dyn LlmService>,
    store: Arc<dyn VectorStore>,
    runner: Arc<dyn SqlRunner>,
    dialect: &'static str,
}

/// Build the shared LLM / vector store / SQL runner. Returns `None` when a required
/// API key is missing (so the app can show a "not configured" state).
fn build_components(settings: &Settings, secrets: &dyn SecretStore) -> Result<Option<Components>> {
    // The DB connection string lives in the encrypted vault, not settings.json.
    // Load it in if the caller didn't already populate it.
    let mut settings = settings.clone();
    if settings.db_connection_string.is_empty() {
        if let Some(c) = secrets.get("db_connection_string")? {
            settings.db_connection_string = c;
        }
    }
    let settings = &settings;

    let model = settings.effective_model();
    let openai_key = secrets.get("openai")?;

    let llm: Arc<dyn LlmService> = match settings.provider.as_str() {
        "openai" => match openai_key.clone() {
            Some(k) => Arc::new(OpenAiLlm::new(k, model)),
            None => return Ok(None),
        },
        "anthropic" => match secrets.get("anthropic")? {
            Some(k) => Arc::new(AnthropicLlm::new(k, model)),
            None => return Ok(None),
        },
        "ollama" => Arc::new(OllamaLlm::new(model)),
        _ => Arc::new(MockLlm::with_default_sql()),
    };

    // Semantic embeddings when an OpenAI key exists (and we're not in offline mock
    // mode), else the local keyword embedder.
    let use_openai_embeddings = settings.provider != "mock" && openai_key.is_some();
    let (embedding, embedder_tag): (Arc<dyn EmbeddingService>, &str) = if use_openai_embeddings {
        (Arc::new(OpenAiEmbedding::new(openai_key.unwrap(), "text-embedding-3-small")), "openai")
    } else {
        (Arc::new(LocalEmbedding::new()), "local")
    };

    // KB file is namespaced by embedder (vector dims) AND database (per-DB schema).
    let kb_file = kb_filename(embedder_tag, settings);
    let store: Arc<dyn VectorStore> = Arc::new(FileVectorStore::new(home().join(kb_file), embedding)?);
    let runner: Arc<dyn SqlRunner> = build_runner(settings)?;

    Ok(Some(Components { llm, store, runner, dialect: dialect_for(&settings.db_kind) }))
}

/// Build a `OpenDbPylot` from settings + vault. Returns `None` when a required API key is
/// missing. (Used for training, schema import, and conversation recording.)
pub fn build_opendbpylot(
    settings: &Settings,
    secrets: &dyn SecretStore,
    conversations: Arc<dyn ConversationStore>,
) -> Result<Option<OpenDbPylot>> {
    let Some(c) = build_components(settings, secrets)? else { return Ok(None) };
    Ok(Some(
        OpenDbPylot::new(c.llm, c.store)
            .with_runner(c.runner)
            .with_conversations(conversations)
            .with_config(OpenDbPylotConfig {
                dialect: c.dialect.to_string(),
                allow_llm_to_see_data: true,
                ..Default::default()
            }),
    ))
}

/// Both runtime objects, sharing the same LLM / store / runner.
pub struct CoreBuild {
    pub opendbpylot: Arc<OpenDbPylot>,
    pub agent: Arc<Agent>,
}

/// Build the shared `OpenDbPylot` + `Agent`. Returns `None` when a required API key is
/// missing. The `Agent` is wired with the `run_sql` + `visualize_data` tools, the
/// SQL-analyst system prompt, and a RAG enhancer over the shared vector store.
pub fn build_core(
    settings: &Settings,
    secrets: &dyn SecretStore,
    conversations: Arc<dyn ConversationStore>,
) -> Result<Option<CoreBuild>> {
    let Some(c) = build_components(settings, secrets)? else { return Ok(None) };

    let opendbpylot = Arc::new(
        OpenDbPylot::new(c.llm.clone(), c.store.clone())
            .with_runner(c.runner.clone())
            .with_conversations(conversations)
            .with_config(OpenDbPylotConfig {
                dialect: c.dialect.to_string(),
                allow_llm_to_see_data: true,
                ..Default::default()
            }),
    );

    // Tools share a single file system for the run_sql → visualize_data hand-off.
    let fs: Arc<dyn FileSystem> = Arc::new(MemoryFileSystem::new());
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(RunSqlTool::new(c.runner.clone(), fs.clone())));
    registry.register(Arc::new(VisualizeDataTool::new(fs)));
    // Agent memory: the model can save durable facts + validated query examples into
    // the same per-DB store the RagEnhancer reads back automatically.
    registry.register(Arc::new(RememberTool::new(c.store.clone())));
    registry.register(Arc::new(SaveQueryTool::new(c.store.clone())));

    let enhancer = Arc::new(RagEnhancer::new(c.store.clone()));
    let agent = Arc::new(
        Agent::new(c.llm.clone(), Arc::new(registry))
            .with_system_prompt(build_sql_system_prompt(c.dialect))
            .with_enhancer(enhancer),
    );

    Ok(Some(CoreBuild { opendbpylot, agent }))
}
