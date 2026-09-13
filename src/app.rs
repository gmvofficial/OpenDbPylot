//! Application wiring: build the runtime objects from saved settings + the secret
//! vault. Used by the server so configuration can change at runtime (not via `.env`).
//!
//! There are two runtime objects, sharing the same LLM / vector store / SQL runner:
//! - `OpenDbPylot` — the single-shot path (used for training, schema import, and
//!   conversation recording).
//! - `Agent` — the tool loop that powers the chat endpoint.
//!
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
use crate::embedding::{
    cache::CachedEmbedding, local::LocalEmbedding, openai::OpenAiEmbedding, EmbeddingService,
};
use crate::llm::{
    anthropic::AnthropicLlm,
    mock::MockLlm,
    ollama::OllamaLlm,
    openai::OpenAiLlm,
    retry::{RetryLlm, RetryPolicy},
    LlmService,
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
        "duckdb" => format!("duckdb:{}", settings.db_path),
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
        "duckdb" => "DuckDB",
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
        #[cfg(feature = "duckdb")]
        "duckdb" => {
            use crate::sqlrunner::duckdb::DuckDbRunner;
            // db_path may be a .duckdb file or ":memory:"; querying CSV/Parquet
            // happens inside the SQL itself (`SELECT * FROM 'data.csv'`).
            Ok(Arc::new(DuckDbRunner::new(settings.db_path.clone())?))
        }
        // Selected but not compiled in — fail loudly rather than silently using SQLite.
        #[cfg(not(feature = "duckdb"))]
        "duckdb" => Err(anyhow::anyhow!(
            "db_kind is 'duckdb' but this build lacks the feature — \
             reinstall with: cargo install opendbpylot --features duckdb"
        )),
        // Default: SQLite
        _ => Ok(Arc::new(SqliteRunner::new(settings.db_path.clone()))),
    }
}

/// Initialize logging/tracing once. Silent by default (only warnings and above);
/// set `OPENDBPYLOT_LOG` to control verbosity, e.g. `OPENDBPYLOT_LOG=debug` to
/// trace the retrieve → prompt → validate → execute → repair pipeline.
/// Idempotent — safe to call from every binary entry point.
pub fn init_tracing() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("OPENDBPYLOT_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt().with_env_filter(filter).with_target(false).without_time().try_init();
}

/// Like [`init_tracing`], but logs to STDERR. Required by `dbpylot mcp`, whose
/// stdout is reserved for the JSON-RPC protocol stream — a single stray log
/// line on stdout would corrupt it. Idempotent, but must run before
/// `init_tracing` on the MCP path (`try_init` is first-wins).
pub fn init_tracing_stderr() {
    use tracing_subscriber::{fmt, EnvFilter};
    let filter = EnvFilter::try_from_env("OPENDBPYLOT_LOG").unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .without_time()
        .try_init();
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

/// Resolve the API key for a provider. The encrypted vault always wins; the
/// standard environment variables (`OPENAI_API_KEY` / `ANTHROPIC_API_KEY`) are
/// a fallback so MCP hosts, containers, and CI can configure `dbpylot` without
/// a vault. Never logged; callers must not print the returned value.
pub fn resolve_api_key(provider: &str, secrets: &dyn SecretStore) -> Result<Option<String>> {
    if let Some(key) = secrets.get(provider)? {
        return Ok(Some(key));
    }
    let var = match provider {
        "openai" => "OPENAI_API_KEY",
        "anthropic" => "ANTHROPIC_API_KEY",
        _ => return Ok(None),
    };
    Ok(std::env::var(var)
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty()))
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
    // For the LLM, an env var may stand in for the vault (so MCP hosts / CI can
    // configure a key without the wizard). The embedding backend decision below
    // deliberately uses the vault only, so an `OPENAI_API_KEY` in the
    // environment can never silently switch "auto" embeddings from local to
    // paid OpenAI (which would also orphan an existing knowledge base).
    let openai_key = resolve_api_key("openai", secrets)?;
    let openai_vault_key = secrets.get("openai")?;

    // 0 = keep the per-provider default timeout.
    let timeout = (settings.llm_timeout_secs > 0)
        .then(|| std::time::Duration::from_secs(settings.llm_timeout_secs));

    let llm: Arc<dyn LlmService> = match settings.provider.as_str() {
        "openai" => match openai_key.clone() {
            Some(k) => {
                let mut p = OpenAiLlm::new(k, model);
                if let Some(t) = timeout {
                    p = p.with_timeout(t);
                }
                Arc::new(p)
            }
            None => return Ok(None),
        },
        "anthropic" => match resolve_api_key("anthropic", secrets)? {
            Some(k) => {
                let mut p = AnthropicLlm::new(k, model);
                if let Some(t) = timeout {
                    p = p.with_timeout(t);
                }
                Arc::new(p)
            }
            None => return Ok(None),
        },
        "ollama" => {
            let mut p = OllamaLlm::new(model);
            if let Some(t) = timeout {
                p = p.with_timeout(t);
            }
            Arc::new(p)
        }
        // Internal/testing provider — reachable via the API for automated smoke
        // tests, but deliberately NOT offered in the UI (see /api/providers).
        "mock" => Arc::new(MockLlm::with_default_sql()),
        // Unknown or unset provider → the app is simply not configured yet.
        // Never silently fall back to the mock: a user must never mistake
        // canned demo output for their real database.
        _ => return Ok(None),
    };

    // Transient-failure retries (timeouts, 429, 5xx) for real providers.
    // The mock stays bare — retrying it would only mask test bugs.
    let llm: Arc<dyn LlmService> = if settings.provider == "mock" {
        llm
    } else {
        Arc::new(RetryLlm::new(llm).with_policy(RetryPolicy {
            max_retries: settings.llm_max_retries,
            ..RetryPolicy::default()
        }))
    };

    // Embedding backend per `settings.embedding_provider`:
    //   "auto"      — OpenAI when a key exists (and not mock mode), else local.
    //   "local"     — offline hashed bag-of-words (no downloads, no key).
    //   "openai"    — hosted embeddings (requires the OpenAI key).
    //   "fastembed" — real local semantic model (needs the compile feature).
    // Non-free backends get a file-backed cache so identical texts are only
    // ever embedded once.
    let cache_path = home().join("cache").join("embeddings.jsonl");
    let openai_embedder = |key: String| -> Arc<dyn EmbeddingService> {
        Arc::new(CachedEmbedding::new(
            Arc::new(OpenAiEmbedding::new(key, "text-embedding-3-small")),
            "openai:text-embedding-3-small",
            Some(cache_path.clone()),
        ))
    };
    let (embedding, embedder_tag): (Arc<dyn EmbeddingService>, &str) =
        match settings.embedding_provider.as_str() {
            "local" => (Arc::new(LocalEmbedding::new()), "local"),
            "openai" => match openai_vault_key.clone() {
                Some(k) => (openai_embedder(k), "openai"),
                None => anyhow::bail!(
                    "embedding_provider is 'openai' but no OpenAI API key is stored"
                ),
            },
            "fastembed" => {
                #[cfg(feature = "fastembed")]
                {
                    (
                        Arc::new(CachedEmbedding::new(
                            Arc::new(crate::embedding::fastembed::FastEmbedding::new()?),
                            "fastembed:all-minilm-l6-v2",
                            Some(cache_path.clone()),
                        )),
                        "fastembed",
                    )
                }
                #[cfg(not(feature = "fastembed"))]
                anyhow::bail!(
                    "embedding_provider is 'fastembed' but this build lacks the feature — \
                     reinstall with: cargo install opendbpylot --features fastembed"
                )
            }
            // "auto" and anything unknown: previous behavior (vault-only, so an
            // env var can't flip the embedder and orphan the knowledge base).
            _ => match openai_vault_key.clone() {
                Some(k) if settings.provider != "mock" => (openai_embedder(k), "openai"),
                _ => (Arc::new(LocalEmbedding::new()), "local"),
            },
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
                // Captures land here for review rather than going straight
                // into retrieval. Shared by the CLI and the web app, so
                // `dbpylot review` sees everything either produced.
                review_queue_path: Some(crate::review::ReviewQueue::path_in(&home())),
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
                // Captures land here for review rather than going straight
                // into retrieval. Shared by the CLI and the web app, so
                // `dbpylot review` sees everything either produced.
                review_queue_path: Some(crate::review::ReviewQueue::path_in(&home())),
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
