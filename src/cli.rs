//! The `dbpylot` command-line app, as a library entry point.
//!
//! [`run`] is called by the `dbpylot` binary and — via [`run_with`] — by the
//! Python and Node bindings, so all three run the *same* in-process CLI:
//!
//!   dbpylot                  # chat with your database (interactive REPL)
//!   dbpylot init             # setup wizard: choose an LLM + database
//!   dbpylot ask "question"   # one-shot: answer a question and exit
//!   dbpylot serve            # launch the web UI (frontend + backend)
//!   dbpylot doctor           # test your LLM + database connections
//!   dbpylot status           # show the current configuration
//!   dbpylot demo             # offline demo on a seeded sample database
//!   dbpylot mcp              # MCP stdio server for agent hosts (OpenPylot, Claude, …)

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use colored::{Color, Colorize};
use indicatif::{ProgressBar, ProgressStyle};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use crate::app;
use crate::conversation::MemoryConversationStore;
use crate::demo::build_demo_opendbpylot;
use crate::secret::{EncryptedFileSecretStore, FileSecretStore, SecretStore};
use crate::settings::Settings;
use crate::sqlrunner::QueryResult;
use crate::opendbpylot::OpenDbPylot;

#[derive(Parser)]
#[command(
    name = "dbpylot",
    about = "opendbpylot — chat with your database in natural language",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Interactive setup wizard: choose an LLM provider and connect a database
    Init,
    /// Ask a single question and exit
    Ask {
        /// The question (quote it, or pass it as trailing words)
        #[arg(trailing_var_arg = true, required = true)]
        question: Vec<String>,
    },
    /// Launch the web UI (a single self-contained frontend + backend)
    Serve {
        /// Don't open a browser — for servers, containers, or remote hosts
        #[arg(long)]
        headless: bool,
        /// Port to bind on 127.0.0.1. Defaults to 8080.
        ///
        /// A host that embeds opendbpylot (OpenPylot's companion registry, for
        /// one) assigns an ephemeral port so several instances can coexist and
        /// so a fixed port is never a source of startup collisions.
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Measure NL→SQL accuracy against a set of reference questions
    ///
    /// Runs each question through the full pipeline, executes the generated SQL
    /// and the reference SQL, and compares the result sets. Reports held-out
    /// accuracy separately from accuracy on questions the model was trained on.
    Eval {
        /// Case file (JSON). Spider-format files load unmodified.
        #[arg(long, value_name = "FILE")]
        cases: Option<std::path::PathBuf>,
        /// Run against the bundled demo database instead of your own
        #[arg(long)]
        demo: bool,
        /// Write the scorecard here, for diffing one run against another
        #[arg(long, value_name = "FILE")]
        out: Option<std::path::PathBuf>,
        /// Compare against an earlier scorecard and fail on a regression
        #[arg(long, value_name = "FILE")]
        baseline: Option<std::path::PathBuf>,
        /// Held-out accuracy may drop by at most this many points vs the baseline
        #[arg(long, default_value_t = 0.0)]
        tolerance: f64,
        /// Run the cases this many times and report the spread
        ///
        /// The model is nondeterministic: three runs of an unchanged pipeline
        /// on the bundled hard set produced 75%, 65% and 60%. One run cannot
        /// separate a real change from that, so any comparison you intend to
        /// act on wants several.
        #[arg(long, default_value_t = 1)]
        runs: usize,
    },
    /// Review question/SQL pairs captured from use before they train the model
    ///
    /// Nothing captured during a conversation reaches retrieval until it is
    /// approved here — an unreviewed example teaches its mistakes to every
    /// later question that retrieves it.
    Review {
        #[command(subcommand)]
        action: Option<ReviewAction>,
    },
    /// Test that the configured LLM and database are reachable
    Doctor,
    /// Show the current configuration (secrets masked)
    Status,
    /// Offline demo on a seeded sample database (no setup needed)
    Demo {
        /// Optional one-shot question; omit for an interactive REPL
        #[arg(trailing_var_arg = true)]
        question: Vec<String>,
    },
    /// Serve opendbpylot as an MCP stdio server (for agent hosts like
    /// OpenPylot, Claude Desktop, or Claude Code)
    Mcp,
    /// Non-interactive configuration (for scripts and host applications)
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ReviewAction {
    /// Show what is waiting (the default)
    List,
    /// Add a pending pair to the training corpus
    Approve {
        /// Its id, or a unique prefix of one
        id: String,
    },
    /// Discard a pending pair so it is never offered again
    Reject {
        /// Its id, or a unique prefix of one
        id: String,
    },
    /// Approve everything waiting
    ApproveAll,
    /// Discard everything waiting
    RejectAll,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Store an LLM API key in the encrypted vault, reading it from stdin so
    /// it never appears in shell history or process arguments.
    /// Example: printf '%s' "$KEY" | dbpylot config set-key openai
    SetKey {
        /// LLM provider: openai or anthropic (ollama is local, no key needed)
        provider: String,
    },
    /// Connect a database non-interactively (for scripts and host apps).
    /// SQLite/DuckDB take a file path argument; Postgres/MySQL read the
    /// connection URL from stdin so the password never lands in argv:
    ///   dbpylot config set-db sqlite /data/app.db
    ///   printf '%s' "$DATABASE_URL" | dbpylot config set-db postgres
    SetDb {
        /// Database kind: sqlite, postgres, mysql, or duckdb
        kind: String,
        /// File path for sqlite/duckdb (omit for postgres/mysql — URL on stdin)
        path: Option<String>,
    },
}

/// Our mascot. 🐘
const ELEPHANT: &str = r#"
          ___       ___
         (   `.   .'   )
          \    `.'    /
           |  o   o  |
           |    ^    |        o p e n d b p y l o t
            \  '-'  /
          ___|     |___
         /             \___
        |    |     |        \___
         \___|_____|            \__
                                   `.___
                                        )
                                    ___.'
"#;

/// Max width for wrapped SQL text inside its box.
const SQL_WRAP: usize = 84;

/// Run the CLI using the process's own arguments. Entry point for the `dbpylot`
/// binary.
pub async fn run() -> Result<()> {
    run_with(std::env::args().collect()).await
}

/// Run the CLI with an explicit argument vector (`args[0]` is the program name).
/// Used by the Python/Node bindings to expose a fully in-process CLI.
pub async fn run_with(args: Vec<String>) -> Result<()> {
    dotenvy::dotenv().ok();
    let cli = Cli::parse_from(args);

    // The MCP server owns stdout for JSON-RPC, so its logs must go to stderr.
    // Both initializers are first-wins (`try_init`), so pick before installing.
    match cli.command {
        Some(Command::Mcp) => crate::app::init_tracing_stderr(),
        _ => crate::app::init_tracing(),
    }

    match cli.command {
        // Bare `dbpylot` → chat with the configured database.
        None => cmd_chat().await,
        Some(Command::Init) => cmd_init().await,
        Some(Command::Ask { question }) => cmd_ask(&question.join(" ")).await,
        Some(Command::Serve { headless, port }) => crate::server::run(!headless, port).await,
        Some(Command::Eval { cases, demo, out, baseline, tolerance, runs }) => {
            cmd_eval(cases.as_deref(), demo, out.as_deref(), baseline.as_deref(), tolerance, runs)
                .await
        }
        Some(Command::Review { action }) => cmd_review(action).await,
        Some(Command::Doctor) => cmd_doctor().await,
        Some(Command::Status) => cmd_status().await,
        Some(Command::Demo { question }) => cmd_demo(&question.join(" ")).await,
        Some(Command::Mcp) => crate::mcp::serve_stdio().await,
        Some(Command::Config { action }) => match action {
            ConfigAction::SetKey { provider } => cmd_set_key(&provider.to_lowercase()),
            ConfigAction::SetDb { kind, path } => cmd_set_db(&kind.to_lowercase(), path.as_deref()),
        },
    }
}

/// `dbpylot config set-key <provider>` — store an API key from stdin.
///
/// Security invariants: the key is only ever read from stdin (never argv, so
/// it can't leak via `ps` or shell history), it is never echoed or logged,
/// and confirmation output shows at most the last 4 characters.
fn cmd_set_key(provider: &str) -> Result<()> {
    use std::io::{IsTerminal, Read};

    match provider {
        "openai" | "anthropic" => {}
        "ollama" => anyhow::bail!(
            "ollama runs locally and does not use an API key — nothing to store"
        ),
        other => anyhow::bail!("unknown provider '{other}' — expected openai or anthropic"),
    }

    // Piped input (the normal path) is read to EOF; an interactive terminal
    // gets a prompt and a single line so the user isn't left hanging.
    let mut input = String::new();
    if std::io::stdin().is_terminal() {
        eprintln!("Paste the {provider} API key and press Enter:");
        std::io::stdin().read_line(&mut input)?;
    } else {
        std::io::stdin().read_to_string(&mut input)?;
    }

    let key = input.trim();
    if key.is_empty() {
        anyhow::bail!("no key provided on stdin");
    }
    if key.split_whitespace().count() != 1 {
        anyhow::bail!("the key contains whitespace — pass exactly one API key");
    }

    let secrets = open_secrets()?;
    secrets.set(provider, key)?;

    // Point the provider at the stored key so the engine actually uses it.
    let settings_path = app::home().join("settings.json");
    let mut settings = Settings::load(&settings_path);
    settings.provider = provider.to_string();
    settings.save(&settings_path)?;

    let masked: String = if key.len() >= 8 {
        format!("…{}", &key[key.len() - 4..])
    } else {
        "…".into()
    };
    println!("stored {provider} API key ({masked}) in the encrypted vault");
    Ok(())
}

/// `dbpylot config set-db <kind> [path]` — connect a database non-interactively.
///
/// SQLite/DuckDB take a file path (stored in settings, made absolute). Postgres
/// and MySQL read the connection URL from stdin — never argv — so the password
/// can't leak via `ps` or shell history; the URL is stored in the encrypted
/// vault, never in `settings.json`. Mirrors what `dbpylot init` does.
fn cmd_set_db(kind: &str, path: Option<&str>) -> Result<()> {
    use std::io::{IsTerminal, Read};

    let settings_path = app::home().join("settings.json");
    let mut settings = Settings::load(&settings_path);

    match kind {
        "postgres" | "postgresql" | "mysql" | "mariadb" => {
            // Connection URL (with password) from stdin → vault.
            let mut url = String::new();
            if std::io::stdin().is_terminal() {
                eprintln!("Paste the {kind} connection URL and press Enter:");
                std::io::stdin().read_line(&mut url)?;
            } else {
                std::io::stdin().read_to_string(&mut url)?;
            }
            let url = url.trim();
            if url.is_empty() {
                anyhow::bail!("no connection URL provided on stdin");
            }
            let secrets = open_secrets()?;
            secrets.set("db_connection_string", url)?;
            settings.db_kind = kind.to_string();
            settings.db_connection_string = url.to_string();
            settings.save(&settings_path)?;
            // Redact credentials before echoing the target back.
            println!("connected {kind} database ({}) — URL stored in the encrypted vault", redact_url(url));
        }
        "sqlite" | "duckdb" => {
            let file = path.ok_or_else(|| {
                anyhow::anyhow!("{kind} needs a file path, e.g. `dbpylot config set-db {kind} /data/app.db`")
            })?;
            let abs = absolute_db_path(file.trim());
            settings.db_kind = kind.to_string();
            settings.db_path = abs.clone();
            settings.db_connection_string.clear();
            settings.save(&settings_path)?;
            println!("connected {kind} database at {abs}");
        }
        other => anyhow::bail!("unknown database kind '{other}' — expected sqlite, postgres, mysql, or duckdb"),
    }
    Ok(())
}

/// Hide the password in a database URL for display, e.g.
/// `postgres://user:secret@host/db` → `postgres://user:***@host/db`.
fn redact_url(url: &str) -> String {
    // Match the `user:password@` credential section and mask the password.
    if let Some(at) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            let creds = &url[scheme_end + 3..at];
            if let Some(colon) = creds.find(':') {
                return format!(
                    "{}{}:***{}",
                    &url[..scheme_end + 3],
                    &creds[..colon],
                    &url[at..]
                );
            }
        }
    }
    url.to_string()
}

/// Build the configured engine (LLM + database) from saved settings + the vault.
/// `None` means the app hasn't been set up yet. Also used by the MCP server so
/// every entry point boots from the exact same configuration.
pub(crate) fn build_configured() -> Result<Option<OpenDbPylot>> {
    let secrets = open_secrets()?;
    let settings = load_settings(&*secrets);
    let conversations = Arc::new(MemoryConversationStore::new());
    app::build_opendbpylot(&settings, &*secrets, conversations)
}

/// Make a database file path absolute (against the current directory) before
/// storing it. The engine may later be launched from a different working
/// directory — e.g. `dbpylot mcp` spawned by an agent host — and a relative
/// path would then point at (and silently create) the wrong file.
fn absolute_db_path(path: &str) -> String {
    if path == ":memory:" || std::path::Path::new(path).is_absolute() {
        return path.to_string();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path).to_string_lossy().into_owned(),
        Err(_) => path.to_string(),
    }
}

/// Shown when the user runs a command before finishing setup.
fn not_configured_hint() {
    println!(
        "  {}\n  Run {} to choose an LLM + database, or {} for the web UI.",
        "dbpylot isn't set up yet.".yellow(),
        "dbpylot init".cyan(),
        "dbpylot serve".cyan()
    );
}

/// Bare `dbpylot` → interactive chat REPL against the configured database.
async fn cmd_chat() -> Result<()> {
    match build_configured()? {
        Some(bot) => {
            print_banner("your database");
            repl(&bot).await
        }
        None => {
            not_configured_hint();
            std::process::exit(1);
        }
    }
}

/// `dbpylot doctor` — check config presence and reachability (no LLM spend).
async fn cmd_doctor() -> Result<()> {
    let secrets = open_secrets()?;
    let settings = load_settings(&*secrets);
    println!("{}", "dbpylot doctor".bold().cyan());

    // LLM provider + key (vault first, env-var fallback — same as the engine).
    let key_ok = settings.provider == "ollama"
        || app::resolve_api_key(&settings.provider, &*secrets).ok().flatten().is_some();
    println!(
        "  LLM provider : {} {}",
        settings.provider.bold(),
        if key_ok { "✓ key present".green() } else { "✗ no key stored".red() }
    );

    // Database reachability.
    match build_configured()? {
        Some(bot) => match bot.test_connection().await {
            Ok(()) => {
                let tables = bot.list_ddl().await.unwrap_or_default().len();
                println!(
                    "  Database     : {} ✓ reachable ({} learned table entr{})",
                    settings.db_kind.bold(),
                    tables,
                    if tables == 1 { "y" } else { "ies" }
                );
            }
            Err(e) => println!("  Database     : {} ✗ {}", settings.db_kind.bold(), e.to_string().red()),
        },
        None => println!("  Database     : {}", "not configured — run `dbpylot init`".yellow()),
    }
    Ok(())
}

/// `dbpylot status` — print the saved configuration (no secrets).
async fn cmd_status() -> Result<()> {
    let secrets = open_secrets()?;
    let settings = load_settings(&*secrets);
    let key_ok = settings.provider == "ollama"
        || app::resolve_api_key(&settings.provider, &*secrets).ok().flatten().is_some();
    let model = settings.effective_model();
    let target = match settings.db_kind.as_str() {
        "postgres" | "postgresql" | "mysql" | "mariadb" => {
            if settings.db_connection_string.is_empty() { "(no connection URL)".into() }
            else { "(connection URL in vault)".into() }
        }
        _ => settings.db_path.clone(),
    };
    println!("{}", "dbpylot status".bold().cyan());
    println!("  provider   : {}", settings.provider);
    println!("  model      : {}", if model.is_empty() { "(default)".into() } else { model });
    println!("  api key    : {}", if key_ok { "stored".green() } else { "missing".red() });
    println!("  database   : {} → {}", settings.db_kind, target);
    println!("  config dir : {}", app::home().display());
    Ok(())
}

/// The encrypted secret vault, shared with the web app (same `~/.opendbpylot`).
fn open_secrets() -> Result<Arc<dyn SecretStore>> {
    let store: Arc<dyn SecretStore> = match std::env::var("OPENDBPYLOT_SECRETS").as_deref() {
        Ok("file") => Arc::new(FileSecretStore::new(app::home().join("secrets.json"))?),
        _ => Arc::new(EncryptedFileSecretStore::new(app::home().join("secrets.enc"))?),
    };
    Ok(store)
}

/// Load saved settings + the DB connection string from the vault (mirrors the
/// server's boot) so terminal `ask` uses the very same configuration as the UI.
fn load_settings(secrets: &dyn SecretStore) -> Settings {
    let mut settings = Settings::load(&app::home().join("settings.json"));
    if settings.db_connection_string.is_empty() {
        if let Ok(Some(c)) = secrets.get("db_connection_string") {
            settings.db_connection_string = c;
        }
    }
    settings
}

/// `opendbpylot ask "..."` — answer once using the user's real configuration.
async fn cmd_ask(question: &str) -> Result<()> {
    let secrets = open_secrets()?;
    let settings = load_settings(&*secrets);
    let conversations = Arc::new(MemoryConversationStore::new());
    match app::build_opendbpylot(&settings, &*secrets, conversations)? {
        Some(bot) => {
            answer(&bot, "cli", question).await;
            Ok(())
        }
        None => {
            not_configured_hint();
            std::process::exit(1);
        }
    }
}

/// `dbpylot eval` — measure accuracy and write a scorecard.
///
/// Auto-training is forced off for the run: a passing case that trains itself
/// would change the score of every later case, making the result depend on case
/// order and impossible to compare between runs.
async fn cmd_eval(
    cases_path: Option<&std::path::Path>,
    demo: bool,
    out: Option<&std::path::Path>,
    baseline: Option<&std::path::Path>,
    tolerance: f64,
    runs: usize,
) -> Result<()> {
    use crate::eval;

    let default_cases = std::path::Path::new("benchmarks/demo.json");
    let cases_path = cases_path.unwrap_or(default_cases);
    let cases = eval::load_cases(cases_path)?;

    let (bot, db, model, dialect) = if demo || cases_path == default_cases {
        let (bot, backend, db) = crate::demo::build_demo_with_runner(false).await?;
        if backend == "offline mock" {
            println!(
                "{}",
                "NOTE: no API key found, so this run uses the offline mock. It checks the \n\
                 harness, not accuracy — the score is not meaningful."
                    .yellow()
            );
        }
        (bot, db as Arc<dyn crate::sqlrunner::SqlRunner>, backend.to_string(), "SQLite".to_string())
    } else {
        let secrets = open_secrets()?;
        let settings = load_settings(&*secrets);
        let dialect = settings.db_kind.clone();
        let model = settings.model.clone();
        let conversations = Arc::new(MemoryConversationStore::new());
        let Some(bot) = app::build_opendbpylot(&settings, &*secrets, conversations)? else {
            not_configured_hint();
            std::process::exit(1);
        };
        let db = app::build_runner(&settings)?;
        (bot, db, model, dialect)
    };

    println!(
        "\n{} {} case(s) from {}\n",
        "Evaluating".bold().cyan(),
        cases.len(),
        cases_path.display()
    );

    let runs = runs.max(1);
    let mut accuracies: Vec<f64> = Vec::with_capacity(runs);
    let mut card: Option<eval::Scorecard> = None;

    for run in 1..=runs {
        if runs > 1 {
            println!("{}", format!("── run {run} of {runs} ──").dimmed());
        }

    let mut index = 0usize;
    let total = cases.len();
    let this_card = eval::run(&bot, db.clone(), &cases, &model, &dialect, |result| {
        index += 1;
        let mark = match result.verdict {
            eval::Verdict::Exact => "PASS".green(),
            eval::Verdict::Equivalent => "pass~".green(),
            eval::Verdict::Wrong => "WRONG".red(),
            eval::Verdict::Failed => "FAIL".red(),
            eval::Verdict::BadReference => "SKIP".yellow(),
        };
        let tag = if result.seen { " (seen)".dimmed() } else { "".normal() };
        println!("  [{index:>3}/{total}] {mark}{tag}  {}", result.question);
        if let Some(sql) = &result.generated_sql {
            println!("        {}", sql.dimmed());
        }
        if let Some(error) = &result.error {
            println!("        {}", error.dimmed());
        }
    })
    .await?;

        accuracies.push(this_card.held_out.accuracy());
        card = Some(this_card);
    }

    // The last run's case detail is kept; the headline number is the mean, so a
    // single lucky or unlucky run cannot stand in for the pipeline's accuracy.
    let mut card = card.expect("at least one run");
    let repeated = eval::Repeated::from_runs(accuracies);

    println!("\n{}", "─".repeat(60));
    if repeated.runs > 1 {
        print!("{}", repeated.summary());
        println!();
    }
    print!("{}", card.summary());
    card.repeated = Some(repeated.clone());

    if let Some(path) = out {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(path, serde_json::to_string_pretty(&card)?)?;
        println!("\nScorecard written to {}", path.display());
    }

    if let Some(path) = baseline {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("could not read baseline {}: {e}", path.display()))?;
        let previous: eval::Scorecard = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("could not parse baseline {}: {e}", path.display()))?;
        let delta = card.regression_against(&previous);

        println!(
            "\nvs baseline: {:+.1} points held-out ({:.1}% → {:.1}%)",
            delta,
            previous.held_out.accuracy() * 100.0,
            card.held_out.accuracy() * 100.0
        );

        // A delta inside the measured noise says nothing, and treating it as a
        // result is how a pipeline gets "tuned" by coin flips.
        if let Some(baseline_spread) = &previous.repeated {
            if !baseline_spread.is_significant(delta) {
                println!(
                    "{}",
                    format!(
                        "  Inconclusive: the baseline's own spread is {:.1} points. \
                         Re-run with --runs to tell this apart from noise.",
                        baseline_spread.spread_points()
                    )
                    .yellow()
                );
                return Ok(());
            }
        }

        if delta < -tolerance {
            anyhow::bail!(
                "held-out accuracy regressed by {:.1} points (tolerance {:.1})",
                -delta,
                tolerance
            );
        }
    }

    Ok(())
}

/// `dbpylot review` — curate what the model learns from use.
///
/// The queue is the only path from a captured conversation into the training
/// corpus, so this is where a human decides which of its own answers the model
/// is allowed to learn from.
async fn cmd_review(action: Option<ReviewAction>) -> Result<()> {
    use crate::review::ReviewQueue;

    let path = ReviewQueue::path_in(&app::home());
    let mut queue = ReviewQueue::load(&path)?;

    match action.unwrap_or(ReviewAction::List) {
        ReviewAction::List => {
            if queue.is_empty() {
                println!("{}", "Nothing waiting for review.".dimmed());
                if queue.approved.is_empty() && queue.rejected.is_empty() {
                    println!(
                        "\nPairs are captured from conversations when {} is on.",
                        "auto_train".cyan()
                    );
                } else {
                    println!(
                        "\n{} approved, {} rejected so far.",
                        queue.approved.len(),
                        queue.rejected.len()
                    );
                }
                return Ok(());
            }

            println!(
                "\n{} — {} waiting\n",
                "Training review".bold().cyan(),
                queue.pending_count()
            );
            for example in &queue.pending {
                // A short id is enough to act on and much easier to type.
                let short = &example.id[..8];
                let rows = if example.row_count == 0 {
                    // Worth flagging: a query that returned nothing is a common
                    // sign of a subtly wrong answer.
                    "0 rows".yellow()
                } else {
                    format!("{} rows", example.row_count).dimmed()
                };
                println!("  {}  {}  {}", short.bold(), rows, example.question);
                println!("    {}", example.sql.dimmed());
            }
            println!(
                "\n  {}  {}",
                "approve:".dimmed(),
                "dbpylot review approve <id>".white()
            );
            println!("  {}  {}", "reject: ".dimmed(), "dbpylot review reject <id>".white());
        }

        ReviewAction::Approve { id } => {
            let Some(resolved) = queue.resolve_id(&id).map(|e| e.id.clone()) else {
                anyhow::bail!("No pending item matching '{id}' (an ambiguous prefix matches none).");
            };
            let Some(pair) = queue.approve(&resolved) else {
                anyhow::bail!("No pending item with id '{resolved}'.");
            };

            // Training needs a configured engine; if there is none, the
            // approval is not recorded, so it can be retried after setup.
            let secrets = open_secrets()?;
            let settings = load_settings(&*secrets);
            let conversations = Arc::new(MemoryConversationStore::new());
            let Some(bot) = app::build_opendbpylot(&settings, &*secrets, conversations)? else {
                not_configured_hint();
                std::process::exit(1);
            };
            bot.train_approved(&pair).await?;
            queue.save(&path)?;

            println!("{} Added to the training corpus:", "✅".green());
            println!("  {}", pair.question);
        }

        ReviewAction::Reject { id } => {
            let Some(resolved) = queue.resolve_id(&id).map(|e| e.id.clone()) else {
                anyhow::bail!("No pending item matching '{id}' (an ambiguous prefix matches none).");
            };
            queue.reject(&resolved);
            queue.save(&path)?;
            println!("{} Rejected — it will not be offered again.", "✅".green());
        }

        ReviewAction::ApproveAll => {
            if queue.is_empty() {
                println!("{}", "Nothing waiting for review.".dimmed());
                return Ok(());
            }
            let secrets = open_secrets()?;
            let settings = load_settings(&*secrets);
            let conversations = Arc::new(MemoryConversationStore::new());
            let Some(bot) = app::build_opendbpylot(&settings, &*secrets, conversations)? else {
                not_configured_hint();
                std::process::exit(1);
            };

            let pairs = queue.approve_all();
            for pair in &pairs {
                bot.train_approved(pair).await?;
            }
            queue.save(&path)?;
            println!("{} Added {} pair(s) to the training corpus.", "✅".green(), pairs.len());
        }

        ReviewAction::RejectAll => {
            let count = queue.reject_all();
            queue.save(&path)?;
            println!("{} Rejected {count} pair(s).", "✅".green());
        }
    }
    Ok(())
}

/// `dbpylot demo [question]` — the offline showcase on a seeded sample DB.
async fn cmd_demo(question: &str) -> Result<()> {
    let (bot, backend) = build_demo_opendbpylot().await?;
    if !question.trim().is_empty() {
        answer(&bot, "cli", question).await;
        return Ok(());
    }
    print_banner(backend);
    repl(&bot).await
}

/// `dbpylot init` — interactive terminal wizard. Writes to the same
/// `settings.json` + encrypted vault the web app uses, then tests the connection.
async fn cmd_init() -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    println!("\n{}\n", "dbpylot init".bold().cyan());

    // 1. LLM provider.
    println!("{}", "1) Choose an LLM provider:".bold());
    println!("   {}  OpenAI          (needs an API key)", "openai".cyan());
    println!("   {}  Anthropic Claude (needs an API key)", "anthropic".cyan());
    println!("   {}  Ollama          (local, no key)", "ollama".cyan());
    let provider = loop {
        let p = prompt(&mut rl, "provider [openai/anthropic/ollama]: ")?.to_lowercase();
        if ["openai", "anthropic", "ollama"].contains(&p.as_str()) {
            break p;
        }
        println!("  {}", "please type openai, anthropic, or ollama".red());
    };

    let secrets = open_secrets()?;
    let mut settings = Settings::load(&app::home().join("settings.json"));
    settings.provider = provider.clone();

    // 2. API key (into the encrypted vault), unless Ollama. A key may already
    // be present — e.g. synced by a host app via `dbpylot config set-key` —
    // in which case leaving the prompt blank keeps it.
    if provider != "ollama" {
        let existing = secrets.get(&provider).ok().flatten().is_some();
        let label = if existing {
            format!("{provider} API key [a key is stored — leave blank to keep it]: ")
        } else {
            format!("{provider} API key: ")
        };
        let key = prompt(&mut rl, &label)?;
        if key.trim().is_empty() {
            if existing {
                println!("  {}", "keeping the stored key".green());
            } else {
                println!("  {}", "no key entered — you can add one later in the web UI".yellow());
            }
        } else {
            secrets.set(&provider, key.trim())?;
        }
    }

    // 3. Optional model override.
    let model = prompt(&mut rl, "model (blank = provider default): ")?;
    settings.model = model.trim().to_string();

    // 4. Database.
    println!("\n{}", "2) Connect a database:".bold());
    let kinds = if cfg!(feature = "duckdb") {
        "sqlite/postgres/mysql/duckdb"
    } else {
        "sqlite/postgres/mysql"
    };
    let db_kind = loop {
        let k = prompt(&mut rl, &format!("database [{kinds}]: "))?.to_lowercase();
        let ok = matches!(k.as_str(), "sqlite" | "postgres" | "postgresql" | "mysql" | "mariadb")
            || (cfg!(feature = "duckdb") && k == "duckdb");
        if ok {
            break k;
        }
        println!("  {}", format!("please type one of: {kinds}").red());
    };
    settings.db_kind = db_kind.clone();

    match db_kind.as_str() {
        "postgres" | "postgresql" | "mysql" | "mariadb" => {
            let url = prompt(&mut rl, "connection URL (e.g. postgres://user:pass@host:5432/db): ")?;
            if !url.trim().is_empty() {
                // The URL holds a password → store it in the vault, not settings.json.
                secrets.set("db_connection_string", url.trim())?;
                settings.db_connection_string = url.trim().to_string();
            }
        }
        _ => {
            let default = if db_kind == "duckdb" { ":memory:" } else { "demo.db" };
            let path = prompt(&mut rl, &format!("file path [{default}]: "))?;
            let path = if path.trim().is_empty() { default.to_string() } else { path.trim().to_string() };
            settings.db_path = absolute_db_path(&path);
        }
    }

    // 5. Save + verify.
    settings.save(&app::home().join("settings.json"))?;
    println!("\n{}", "Saved. Testing the connection…".dimmed());

    let conversations = Arc::new(MemoryConversationStore::new());
    match app::build_opendbpylot(&settings, &*secrets, conversations)? {
        Some(bot) => match bot.test_connection().await {
            Ok(()) => match bot.train_from_schema().await {
                Ok(n) => {
                    println!("  {} connected — learned {n} table(s).", "✓".green().bold());
                    println!("\nStart chatting:  {}", "dbpylot".cyan());
                    println!("Or ask once:     {}", "dbpylot ask \"how many rows are in each table?\"".cyan());
                }
                Err(e) => println!(
                    "  {} connected to the database, but couldn't import the schema:\n     {e}\n  \
                     This is often a rejected API key (the schema is embedded via your LLM \
                     provider). Check the key and re-run {}.",
                    "!".yellow().bold(),
                    "dbpylot init".cyan()
                ),
            },
            Err(e) => println!("  {} configured, but couldn't reach the database: {e}", "!".yellow().bold()),
        },
        None => println!(
            "  {} saved, but no API key is stored yet — add one with {} or in the web UI.",
            "!".yellow().bold(),
            "dbpylot init".cyan()
        ),
    }
    Ok(())
}

/// Read a line with a prompt, trimming the trailing newline.
fn prompt(rl: &mut DefaultEditor, label: &str) -> Result<String> {
    match rl.readline(label) {
        Ok(s) => Ok(s),
        Err(ReadlineError::Interrupted | ReadlineError::Eof) => {
            println!("\n{}", "setup cancelled".dimmed());
            std::process::exit(130);
        }
        Err(e) => Err(e.into()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// REPL
// ─────────────────────────────────────────────────────────────────────────────

async fn repl(opendbpylot: &OpenDbPylot) -> Result<()> {
    let mut rl = DefaultEditor::new()?;

    loop {
        // The "ask" box: an open-topped box around the input line.
        println!(
            "  {}",
            format!("╭─ ask {}╮", "─".repeat(40)).bright_green()
        );
        let read = rl.readline("  │ ❯ ");
        println!(
            "  {}",
            format!("╰{}╯", "─".repeat(46)).bright_green()
        );

        match read {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(input);

                if input.starts_with('/') {
                    if handle_command(opendbpylot, input).await {
                        break;
                    }
                } else {
                    // A single conversation per CLI session enables follow-ups.
                    answer(opendbpylot, "cli", input).await;
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                println!("{}", "  bye! 🐘".bright_magenta());
                break;
            }
            Err(e) => {
                eprintln!("{} {e}", "input error:".red());
                break;
            }
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Commands (return true to quit)
// ─────────────────────────────────────────────────────────────────────────────

async fn handle_command(opendbpylot: &OpenDbPylot, input: &str) -> bool {
    let mut parts = input.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("").trim();

    match cmd {
        "/quit" | "/exit" | "/q" => {
            println!("{}", "  bye! 🐘".bright_magenta());
            return true;
        }
        "/help" | "/?" | "/h" => print_help(),
        "/clear" => print!("\x1B[2J\x1B[H"),
        "/examples" => print_examples(),
        "/tables" => show_tables(opendbpylot).await,
        "/schema" => learn_schema(opendbpylot).await,
        "/show" | "/training" => show_training(opendbpylot).await,
        "/run" => run_raw_sql(opendbpylot, rest).await,
        "/train" => train_cmd(opendbpylot, rest).await,
        other => {
            println!(
                "  {} unknown command {}. Type {} for help.",
                "✗".red(),
                other.yellow(),
                "/help".cyan()
            );
        }
    }
    false
}

async fn train_cmd(opendbpylot: &OpenDbPylot, rest: &str) {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let kind = parts.next().unwrap_or("");
    let body = parts.next().unwrap_or("").trim();

    if body.is_empty() && !kind.is_empty() {
        println!("  {} nothing to train. See {}.", "✗".red(), "/help".cyan());
        return;
    }

    let result = match kind {
        "ddl" => opendbpylot.train_ddl(body).await,
        "doc" | "documentation" => opendbpylot.train_documentation(body).await,
        "sql" => match body.split_once('|') {
            Some((q, sql)) => opendbpylot.train_question_sql(q.trim(), sql.trim()).await,
            None => {
                println!(
                    "  {} usage: {}",
                    "✗".red(),
                    "/train sql <question> | <sql>".cyan()
                );
                return;
            }
        },
        _ => {
            println!(
                "  {} usage: {} | {} | {}",
                "✗".red(),
                "/train ddl <...>".cyan(),
                "/train doc <...>".cyan(),
                "/train sql <q> | <sql>".cyan()
            );
            return;
        }
    };

    match result {
        Ok(()) => println!("  {} trained ({kind}).", "✓".green()),
        Err(e) => println!("  {} {e}", "error:".red()),
    }
}

async fn run_raw_sql(opendbpylot: &OpenDbPylot, sql: &str) {
    if sql.is_empty() {
        println!("  {} usage: {}", "✗".red(), "/run <SQL>".cyan());
        return;
    }
    match opendbpylot.run_sql(sql).await {
        Ok(result) => print_box("RESULT", &result_lines(&result), Color::Cyan, None),
        Err(e) => println!("  {} {e}", "error:".red()),
    }
}

async fn show_tables(opendbpylot: &OpenDbPylot) {
    match opendbpylot
        .run_sql("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name")
        .await
    {
        Ok(result) => {
            if result.rows.is_empty() {
                println!("  {}", "(no tables)".dimmed());
                return;
            }
            let mut lines = Vec::new();
            for row in &result.rows {
                lines.push(format!("▣ {}", row[0]));
                if let Some(ddl) = row.get(1) {
                    for line in ddl.lines() {
                        lines.push(format!("  {}", line.trim_end()));
                    }
                }
            }
            print_box("TABLES", &lines, Color::Blue, None);
        }
        Err(e) => println!("  {} {e}", "error:".red()),
    }
}

async fn learn_schema(opendbpylot: &OpenDbPylot) {
    match opendbpylot.train_from_sqlite_schema().await {
        Ok(n) => println!(
            "  {} learned {} table(s) from the live database schema.",
            "✓".green(),
            n
        ),
        Err(e) => println!("  {} {e}", "error:".red()),
    }
}

async fn show_training(opendbpylot: &OpenDbPylot) {
    let ddl = opendbpylot.list_ddl().await.unwrap_or_default();
    let docs = opendbpylot.list_documentation().await.unwrap_or_default();
    let qsql = opendbpylot.list_question_sql().await.unwrap_or_default();

    let mut lines = vec![format!(
        "{} DDL · {} docs · {} question/SQL pairs",
        ddl.len(),
        docs.len(),
        qsql.len()
    )];
    for d in &ddl {
        lines.push(format!("DDL    {}", d.lines().next().unwrap_or("")));
    }
    for d in &docs {
        lines.push(format!("DOC    {d}"));
    }
    for p in &qsql {
        lines.push(format!("Q→SQL  {}", p.question));
    }
    print_box("TRAINING DATA", &lines, Color::Blue, None);
}

// ─────────────────────────────────────────────────────────────────────────────
// Asking a question
// ─────────────────────────────────────────────────────────────────────────────

async fn answer(opendbpylot: &OpenDbPylot, conversation_id: &str, question: &str) {
    let spinner = make_spinner();
    let result = opendbpylot.ask_in_conversation(conversation_id, question).await;
    spinner.finish_and_clear();

    match result {
        Ok(ans) => {
            print_box("SQL", &wrap(&ans.sql, SQL_WRAP), Color::Magenta, Some(Color::Yellow));
            if ans.repairs_used > 0 {
                println!(
                    "  {}",
                    format!("(self-repaired after {} failed attempt(s))", ans.repairs_used).dimmed()
                );
            }
            match ans.result {
                Some(rows) => print_box("RESULT", &result_lines(&rows), Color::Cyan, None),
                None => println!(
                    "  {}",
                    "(not run — not a read query or no database)".dimmed()
                ),
            }
            // Optional natural-language answer (when summaries are enabled).
            if let Some(answer) = &ans.answer {
                print_box("ANSWER", &wrap(answer, SQL_WRAP), Color::Green, None);
            }
            println!();
        }
        Err(e) => println!("  {} {e}\n", "error:".red().bold()),
    }
}

fn make_spinner() -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("  {spinner} {msg}")
            .unwrap()
            .tick_strings(&["🐘  ", " 🐘 ", "  🐘", " 🐘 "]),
    );
    pb.set_message("opendbpylot is thinking...".dimmed().to_string());
    pb.enable_steady_tick(Duration::from_millis(180));
    pb
}

// ─────────────────────────────────────────────────────────────────────────────
// Boxes & pretty output
// ─────────────────────────────────────────────────────────────────────────────

/// Draw a titled, bordered box around `lines`.
///
/// `border` colors the frame; `content` (if set) colors the text inside.
fn print_box(title: &str, lines: &[String], border: Color, content: Option<Color>) {
    let title_len = title.chars().count();
    let mut width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    width = width.max(title_len + 1);
    let span = width + 2; // chars between the corner pieces

    // Top border with the title embedded: ╭─ TITLE ───────╮
    let fill = span - (title_len + 3);
    let top = format!("╭─ {title} {}╮", "─".repeat(fill));
    println!("  {}", top.color(border));

    // Content rows.
    for line in lines {
        let pad = width - line.chars().count();
        let padded = format!("{line}{}", " ".repeat(pad));
        let body = match content {
            Some(c) => padded.color(c).to_string(),
            None => padded,
        };
        println!("  {} {} {}", "│".color(border), body, "│".color(border));
    }

    // Bottom border.
    println!("  {}", format!("╰{}╯", "─".repeat(span)).color(border));
}

/// Turn a query result into aligned plain-text lines (for putting inside a box).
fn result_lines(result: &QueryResult) -> Vec<String> {
    if result.columns.is_empty() {
        return vec!["(statement ran; no rows)".to_string()];
    }

    let mut widths: Vec<usize> = result.columns.iter().map(|c| c.chars().count()).collect();
    for row in &result.rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let pad = |s: &str, w: usize| format!("{s:<width$}", width = w);

    let mut lines = Vec::new();
    lines.push(
        result
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| pad(c, widths[i]))
            .collect::<Vec<_>>()
            .join("  "),
    );
    lines.push(
        widths
            .iter()
            .map(|w| "─".repeat(*w))
            .collect::<Vec<_>>()
            .join("  "),
    );
    for row in &result.rows {
        lines.push(
            row.iter()
                .enumerate()
                .map(|(i, c)| pad(c, widths[i]))
                .collect::<Vec<_>>()
                .join("  "),
        );
    }
    lines.push(format!("{} row(s)", result.rows.len()));
    lines
}

/// Word-wrap text to a max width, preserving existing line breaks.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines() {
        if raw.chars().count() <= width {
            out.push(raw.to_string());
            continue;
        }
        let mut current = String::new();
        for word in raw.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.chars().count() + 1 + word.chars().count() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                out.push(std::mem::take(&mut current));
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Banner & help
// ─────────────────────────────────────────────────────────────────────────────

fn print_banner(backend: &str) {
    println!("{}", ELEPHANT.bright_magenta());
    println!(
        "  {}  {}",
        "opendbpylot".bright_white().bold(),
        "— chat with your database".dimmed()
    );
    println!("  {} {}", "backend:".dimmed(), backend.bright_green());
    println!(
        "  {} {} {}\n",
        "type a question, or".dimmed(),
        "/help".cyan(),
        "for commands".dimmed()
    );
}

fn print_help() {
    let rows = [
        ("<your question>", "ask in plain English → SQL + results"),
        ("/run <SQL>", "run raw SQL directly"),
        ("/tables", "show database tables + schema"),
        ("/schema", "auto-train from the live DB schema"),
        ("/show", "list current training data"),
        ("/train ddl <...>", "teach a table definition"),
        ("/train doc <...>", "teach a business note"),
        ("/train sql <q> | <sql>", "teach a question/SQL example"),
        ("/examples", "show example questions"),
        ("/clear", "clear the screen"),
        ("/help", "show this help"),
        ("/quit", "exit"),
    ];
    let lines: Vec<String> = rows
        .iter()
        .map(|(c, d)| format!("{c:<24} {d}"))
        .collect();
    print_box("COMMANDS", &lines, Color::Magenta, None);
}

fn print_examples() {
    let lines: Vec<String> = [
        "How many users are there per country?",
        "What are the names of users from the USA?",
        "How many users in total?",
        "List users created after 2024-06-01",
    ]
    .iter()
    .map(|q| format!("• {q}"))
    .collect();
    print_box("TRY ASKING", &lines, Color::Magenta, None);
}
