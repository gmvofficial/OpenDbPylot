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
```

`dbpylot init` walks you through choosing a provider (OpenAI, Anthropic, or a local Ollama
model), stores the API key in an encrypted vault, and connects a database. If you prefer a UI,
`dbpylot serve` opens the same setup in the browser.

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
    cli.rs           the dbpylot CLI (init, chat, ask, serve, doctor, status, demo)
    server.rs        axum web server with the embedded UI
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
