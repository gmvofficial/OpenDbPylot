# opendbpylot

Ask your database questions in plain English and get back SQL, results, and charts.

[![crates.io](https://img.shields.io/crates/v/opendbpylot.svg)](https://crates.io/crates/opendbpylot)
[![docs.rs](https://docs.rs/opendbpylot/badge.svg)](https://docs.rs/opendbpylot)
[![license](https://img.shields.io/crates/l/opendbpylot.svg)](LICENSE)

opendbpylot turns a natural language question into SQL, runs it against your database, and
returns the rows. It uses retrieval augmented generation (RAG): you train it on your schema
and business rules, and it retrieves the relevant pieces so the model can write accurate SQL.
Every generated query is validated against your real schema and repaired automatically before
it runs.

## Install

opendbpylot is published to three registries. Each one installs the same `dbpylot` command,
which includes a setup wizard, a terminal chat, and an embedded web app. No Rust toolchain is
needed for the Python or Node packages; they ship prebuilt binaries.

| Registry | Command | Requires |
| --- | --- | --- |
| crates.io | `cargo install opendbpylot` | Rust toolchain |
| PyPI | `pip install opendbpylot` | Python 3.9 or newer |
| npm | `npm install -g opendbpylot` | Node 16 or newer |

Once it is installed:

```bash
dbpylot init      # configure an LLM provider and a database
dbpylot           # chat with your database in the terminal
dbpylot serve     # open the web app in your browser
dbpylot ask "revenue by product category last quarter"
dbpylot doctor    # check your LLM and database connections
dbpylot status    # show the current configuration
dbpylot demo      # try it offline on a sample database
dbpylot mcp       # serve as an MCP server for agent hosts
dbpylot eval      # measure NL→SQL accuracy against reference questions
dbpylot review    # approve what the model learns from your conversations
```

`dbpylot init` walks you through choosing a provider (OpenAI, Anthropic, or a local Ollama
model), stores the API key in an encrypted vault, and connects a database. If you prefer a UI,
`dbpylot serve` opens the same setup in the browser.

## Measuring accuracy

Accuracy claims are worth what their measurement is worth, so there is a harness:

```bash
dbpylot eval --cases benchmarks/hard.json --demo --runs 3
```

It runs each question through the full pipeline, executes both the generated and the reference
SQL, and compares result sets — so a correct query written differently still counts. Questions
the model was trained on are scored separately from held-out ones, because mixing them overstates
capability.

**Use `--runs 3` or more.** The model is nondeterministic: three runs of an unchanged pipeline on
the bundled hard set scored 75%, 65% and 60%. One run cannot tell a real change from that, and the
harness now says so — a `--baseline` comparison whose delta falls inside the baseline's own spread
is reported as inconclusive rather than as a result.

The current held-out baseline on `benchmarks/hard.json` is **58.3%, mean of three runs (55–60%)**.
The older `benchmarks/demo.json` scores 100% and is saturated; it checks plumbing, not capability.

## Reviewing what it learns

A question whose SQL returned rows is not necessarily a question answered correctly, and a wrong
example teaches its mistake to everything that later retrieves it. So captured question/SQL pairs
wait for a human:

```bash
dbpylot review                  # what is waiting
dbpylot review approve <id>     # add it to the training corpus
dbpylot review reject <id>      # discard it, permanently
```

Nothing reaches retrieval until it is approved. Rejected pairs never come back.

## Features

* Natural language to SQL over your own database.
* Automatic query repair. Hallucinated columns and syntax errors are caught against your real
  schema and corrected before the query runs.
* Hybrid retrieval that combines keyword search (BM25) and vector similarity, so the right
  tables reach the prompt even on large schemas.
* Four databases: SQLite, PostgreSQL, MySQL, and DuckDB (query CSV and Parquet files directly).
* Three LLM providers: OpenAI, Anthropic, or a fully local Ollama model. API keys are stored
  encrypted, never in plain text.
* One binary for both the CLI and the web UI. The frontend is compiled in.
* Read only by default. Only `SELECT` and `WITH` queries run automatically; writes are blocked.
* Resilient in production: request timeouts, automatic retries on transient failures, and a
  prompt budget that keeps large schemas within the model's context window.

## Web app

`dbpylot serve` starts a local web app at `http://127.0.0.1:8080` with a chat interface that
renders SQL, result tables, and Plotly charts. The whole UI is a Lit web component compiled
into the binary, so there is nothing extra to deploy. From the sidebar you can:

* Configure the LLM and database in Settings, then connect with one click.
* Add documentation, table definitions, and example queries to train the model.
* Keep multiple conversations that persist across restarts.

On first run the app opens Settings and stays disconnected until you configure a provider and a
database, so it never returns fake answers.

## Use as an MCP server

`dbpylot mcp` serves opendbpylot's capabilities over the Model Context Protocol (stdio), so
any MCP host — OpenPylot, Claude Desktop, Claude Code, or the MCP Inspector — can chat with
your database. Add this to your host's MCP server configuration:

```json
{ "name": "dbpylot", "transport": "stdio", "command": "dbpylot", "args": ["mcp"] }
```

(For Claude Desktop the equivalent is `"dbpylot": { "command": "dbpylot", "args": ["mcp"] }`
under `mcpServers`.)

Six tools are exposed, and their names are a stable contract:

| Tool | What it does |
| --- | --- |
| `ask_database` | Natural-language question → SQL (RAG + self-repair) → rows |
| `run_sql` | Run a read-only SELECT/WITH query; writes are rejected |
| `list_schema` | Learned DDL, documentation notes, and example questions |
| `refresh_schema` | Re-introspect the live database schema |
| `train` | Teach DDL, documentation, or question→SQL examples |
| `health` | Report configured/connected status |

Notes:

* The server uses the same configuration as the CLI (`dbpylot init`). If it isn't set up
  yet, it still starts; tools return an error asking you to run `dbpylot init` — no restart
  needed afterwards.
* Host applications can configure the engine without the interactive wizard, secrets piped
  through stdin so they never touch argv, `ps`, or shell history:

  ```bash
  printf '%s' "$OPENAI_API_KEY" | dbpylot config set-key openai
  dbpylot config set-db sqlite /data/app.db            # file path for sqlite/duckdb
  printf '%s' "$DATABASE_URL" | dbpylot config set-db postgres   # URL on stdin for postgres/mysql
  ```

  The API key can also come from `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` in the environment
  (the vault always wins if both exist). OpenPylot's `pylot add dbpylot` uses `set-key` to
  share its key automatically.
* Query execution is read-only: `run_sql` enforces the same SELECT/WITH-only gate as the
  web app, checked before the database is ever touched.
* Logs go to stderr (`OPENDBPYLOT_LOG=debug` to trace); stdout carries only the protocol.
* Tool error messages redact credentials, so a database driver error can never leak the
  connection password to the host.
* Results are capped at 200 rows per call, with `row_count` and `truncated` reported.

## Use as a library

Besides the CLI, opendbpylot is a library in all three ecosystems.

Rust:

```rust
use opendbpylot::opendbpylot::OpenDbPylot;

let answer = bot.ask("How many orders shipped last week?").await?;
println!("{}", answer.sql);
```

Python:

```python
import opendbpylot

bot = opendbpylot.OpenDbPylot()
result = bot.ask("orders per country")
print(result["sql"])
```

Node:

```js
import { OpenDbPylot } from "opendbpylot";

const bot = new OpenDbPylot();
console.log(bot.ask("orders per country").sql);
```

See [python/README.md](python/README.md) and [node/README.md](node/README.md) for the full API.

## How it works

```text
question
  1. retrieve related schema, docs, and example queries   [vector store]
  2. assemble a prompt within a token budget              [prompt]
  3. generate SQL                                         [llm]
  4. validate against the schema, repair on failure      [schema + repair loop]
  5. run read only and return rows                       [sql runner]
```

You train the model once with three kinds of material:

```rust
bot.train_ddl("CREATE TABLE orders (id INTEGER, customer_id INTEGER, status TEXT, total REAL);").await?;
bot.train_documentation("Revenue excludes orders with status 'cancelled' or 'refunded'.").await?;
bot.train_question_sql("orders per status", "SELECT status, COUNT(*) FROM orders GROUP BY status;").await?;
```

Training material is embedded and stored in a vector store. At query time the most relevant
tables, docs, and examples are retrieved and assembled into the prompt.

## Architecture

Every layer is a Rust trait, so backends are interchangeable without touching the core.

| Layer | Trait | Implementations |
| --- | --- | --- |
| LLM | `LlmService` | OpenAI, Anthropic, Ollama |
| Embeddings | `EmbeddingService` | local, OpenAI, fastembed |
| Vector store | `VectorStore` | in memory, file, Qdrant |
| SQL runner | `SqlRunner` | SQLite, PostgreSQL, MySQL, DuckDB |
| Conversations | `ConversationStore` | in memory, file |

Optional Cargo features: `qdrant`, `keychain`, `fastembed`, and `duckdb`. PostgreSQL and MySQL
are on by default via the `remote-db` feature; use `--no-default-features` for SQLite only.

## Project layout

```text
opendbpylot/
  src/
    cli.rs           the dbpylot CLI (init, chat, ask, serve, doctor, status, demo, mcp)
    server.rs        axum web server with the embedded UI
    mcp.rs           MCP stdio server for agent hosts (dbpylot mcp)
    opendbpylot.rs   orchestrator: train, ask, and the repair loop
    app.rs           builds the engine from settings and the secret vault
    prompt.rs        assembles the prompt within a token budget
    schema.rs        pre execution schema validation
    retrieval.rs     hybrid BM25 and vector ranking
    sql.rs           SQL extraction and the read only gate
    llm/             LlmService: openai, anthropic, ollama, retry
    embedding/       EmbeddingService: local, openai, cache, fastembed
    vectorstore/     VectorStore: memory, file, qdrant
    sqlrunner/       SqlRunner: sqlite, postgres, mysql, duckdb
    conversation.rs  conversation history (memory and file)
    secret.rs        encrypted secret vault
    settings.rs      user settings
    bin/dbpylot.rs   thin entry point that calls cli::run()
  frontends/         TypeScript and Lit web component, compiled into the binary
  python/            Python bindings (pyo3 and maturin)
  node/              Node bindings (napi-rs)
  tests/             cross backend integration tests
  examples/          accuracy evaluation harness
  .github/workflows/ release automation for crates.io, PyPI, and npm
```

## Development

```bash
cargo test                    # unit tests
cargo test --test backends    # cross backend integration tests
cargo run -- serve            # run the web app from source
cargo run --example eval      # accuracy benchmark (needs an OpenAI key)
```

Full documentation is in [DOCUMENTATION.md](DOCUMENTATION.md).

## License

Apache 2.0. See [LICENSE](LICENSE).
