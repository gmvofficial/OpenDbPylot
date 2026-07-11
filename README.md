# 🦀 opendbpylot

**Natural-language → SQL → answers, in Rust.**

[![crates.io](https://img.shields.io/crates/v/opendbpylot.svg)](https://crates.io/crates/opendbpylot)
[![docs.rs](https://docs.rs/opendbpylot/badge.svg)](https://docs.rs/opendbpylot)
[![license](https://img.shields.io/crates/l/opendbpylot.svg)](LICENSE)

**opendbpylot** turns a natural-language question into SQL, runs it on your database, and shows
you the results. It uses **Retrieval-Augmented Generation (RAG)** — it "learns" your database from
training material and retrieves the relevant pieces to help an LLM write accurate SQL.

## Highlights

- **Ask in plain English** → generated SQL → executed → results, with tables and charts.
- **Self-repairing SQL** — a bad column or syntax error is validated against your schema and
  fed back to the model for correction before it ever reaches the database.
- **Hybrid retrieval** — BM25 keyword search fused with vector similarity, so the right tables
  and examples land in the prompt even on messy schemas.
- **Many databases** — SQLite, PostgreSQL, MySQL, and DuckDB (query CSV/Parquet files directly).
- **Your choice of LLM** — OpenAI, Anthropic, or fully-local Ollama. Keys stored **encrypted**.
- **Terminal or browser** — a single `dbpylot` command with a chat REPL, a setup wizard, and an
  embedded web UI (frontend + backend in one binary).
- **Read-only by design** — only `SELECT`/`WITH` queries auto-run; writes are blocked.
- **Robust** — request timeouts, retry-with-backoff on transient failures, and a token budget
  that keeps large schemas from overflowing the model's context.

> A compact, self-contained implementation — small enough to read end-to-end.

📖 **Full project documentation:** [DOCUMENTATION.md](DOCUMENTATION.md)

---

## Install

**As an app** (installs the `dbpylot` command):

```bash
cargo install opendbpylot

dbpylot init                # setup wizard: choose an LLM + database
dbpylot                     # chat with your database (interactive REPL)
dbpylot ask "how many orders per country?"   # one-shot query
dbpylot serve               # launch the web UI (frontend + backend), opens your browser
dbpylot doctor              # test your LLM + database connections
dbpylot status              # show the current configuration
dbpylot demo                # offline demo on a seeded sample database
```

Run `dbpylot init` once to pick a provider (OpenAI / Anthropic / Ollama), paste a key
(stored encrypted), and connect a database — then `dbpylot` chats with it. Prefer a GUI?
`dbpylot serve` opens the same setup in the browser; the web UI is **embedded in the
binary**, so it's a single self-contained frontend+backend with nothing extra to deploy.

**As a library** in your own Rust project:

```bash
cargo add opendbpylot
# optional backends:
cargo add opendbpylot --features qdrant     # Qdrant vector store
cargo add opendbpylot --features keychain   # OS keychain for secrets
cargo add opendbpylot --features fastembed  # local semantic embeddings (ONNX; ~80 MB model on first use)
cargo add opendbpylot --features duckdb     # DuckDB backend — query local CSV/Parquet directly
```

By default the `remote-db` feature is on (PostgreSQL + MySQL support via `sqlx`). Disable it
with `--no-default-features` if you only need SQLite.

---

## Quick start (from source)

```bash
git clone https://github.com/gmvofficial/OpenDbPylot
cd OpenDbPylot

# 1. Set up an LLM + database, then chat with it
cargo run -- init
cargo run                       # interactive chat REPL
cargo run -- ask "how many orders per country?"

# 2. Prefer a browser? Launch the web UI
cargo run -- serve

# 3. Offline demo (seeded sample DB, no key needed)
cargo run -- demo
```

### Self-serve app (no .env needed)

`dbpylot serve` (or `cargo run -- serve`) → opens http://127.0.0.1:8080. From the **sidebar**:
- **Settings** — pick an LLM (OpenAI / Anthropic / Ollama), paste an API key
  (stored in a **secret vault**: AES-256-GCM file, or the OS keychain via
  `OPENDBPYLOT_SECRETS`), choose a database, and **Save & connect** — which tests the
  connection and imports the schema automatically.
- **Train** — add documentation / DDL / question→SQL examples, or **Re-import schema**
  after your database structure changes.
- **Conversations** — multiple chats that **persist and remember history**; create,
  switch, and delete them like a normal chat tool.

On first run the app opens **Settings** and stays in a clear *not-connected* state
until you pick an LLM provider (paste an API key, or choose Ollama for a fully
local, keyless setup) and connect a database — no fake demo answers, ever.

### Web component

The UI is a TypeScript + **Lit** `<opendbpylot-chat>` custom element (in `frontends/`)
that streams **rich UI components** over SSE — rendering live SQL, tables, and **Plotly
charts**. It's compiled to a single bundle and **embedded in the binary** at build time,
so `dbpylot serve` ships the whole app in one executable. To rebuild it from source:

```bash
cd frontends && npm install && npm run build
```

Embed it in your own page: `<opendbpylot-chat sse-endpoint="/api/opendbpylot/v2/chat_sse" theme="dark"></opendbpylot-chat>`.

---

## Interactive CLI 🐘

`dbpylot` (after `dbpylot init`) launches a friendly elephant REPL where you can type
questions and run commands against your database. It has line editing, history (↑/↓),
colored output, and an animated "thinking" elephant while it works.

```
  │ ❯ How many users are there per country?

  SQL
  SELECT country, COUNT(*) AS user_count FROM users GROUP BY country ORDER BY user_count DESC;

  RESULT
  country  user_count
  ───────  ──────────
  USA      2
  UK       1
  ...
```

**Commands:**

| Command | Does |
|---|---|
| `<your question>` | ask in plain English → SQL + results |
| `/run <SQL>` | run raw SQL directly |
| `/tables` | show database tables + schema |
| `/show` | list current training data |
| `/train ddl <...>` | teach a table definition |
| `/train doc <...>` | teach a business note |
| `/train sql <q> \| <sql>` | teach a question/SQL example |
| `/examples` | show example questions |
| `/clear` · `/help` · `/quit` | screen / help / exit |

One-shot (scripting): `dbpylot ask "how many users in total?"`

---

## What you get

Ask `"How many users are there per country?"` and get:

```
Generated SQL:
SELECT country, COUNT(*) AS user_count FROM users GROUP BY country ORDER BY user_count DESC;

Results:
country | user_count
--------------------
USA | 2
UK | 1
Spain | 1
Canada | 1
```

The web UI renders the SQL and the result table in a chat interface.

---

## How it works (the RAG pipeline)

```text
question
  → retrieve relevant context   (similar Q/SQL, related DDL, related docs)   [vectorstore]
  → build a prompt from context                                              [prompt]
  → ask the LLM to write SQL                                                 [llm]
  → extract clean SQL from the reply                                         [sql]
  → run it on the database                                                   [sqlrunner]
  → return rows
```

You first **train** the model:

```rust
opendbpylot.train_ddl("CREATE TABLE users (id INTEGER, name TEXT, country TEXT, created_at TEXT);").await?;
opendbpylot.train_documentation("'country' is the user's country name.").await?;
opendbpylot.train_question_sql("How many users from each country?",
                         "SELECT country, COUNT(*) FROM users GROUP BY country;").await?;
```

Then **ask**:

```rust
let answer = opendbpylot.ask("How many users are there per country?").await?;
println!("{}", answer.sql);
```

---

## Architecture: swappable boxes

Every layer is a Rust **trait**, so you can swap providers without touching the core:

| Layer        | Trait              | Implementations included                          |
|--------------|--------------------|---------------------------------------------------|
| LLM          | `LlmService`       | `OpenAiLlm`, `AnthropicLlm`, `OllamaLlm`, `MockLlm` |
| Embeddings   | `EmbeddingService` | `OpenAiEmbedding`, `LocalEmbedding`               |
| Vector store | `VectorStore`      | `MemoryVectorStore`, `FileVectorStore` (persistent), `QdrantVectorStore` (`--features qdrant`) |
| SQL runner   | `SqlRunner`        | `SqliteRunner`, `PostgresRunner` + `MySqlRunner` (`remote-db`, default), `DuckDbRunner` (`--features duckdb`; query CSV/Parquet directly) |
| Conversations| `ConversationStore`| `MemoryConversationStore`, `FileConversationStore` (persistent) |

**Beyond generation:** a **self-repair loop** (schema validation + error feedback),
**hybrid BM25 + vector retrieval**, auto-training from the live DB schema,
`intermediate_sql` (let the model peek at data, opt-in via `allow_llm_to_see_data`), a
prompt **token budget**, LLM **retries/timeouts**, and **streaming** responses over SSE
(`POST /api/opendbpylot/v2/chat_sse`, rendered live in the web UI). Set
`OPENDBPYLOT_LOG=debug` to trace the retrieval → SQL → execution pipeline.

The `OpenDbPylot` struct (in [`src/opendbpylot.rs`](src/opendbpylot.rs)) wires them together — it's the
central orchestrator that the whole pipeline hangs off of.

---

## Project layout

```
opendbpylot/
├── src/
│   ├── lib.rs            crate root / module list
│   ├── bin/dbpylot.rs    the `dbpylot` CLI (init / chat / ask / serve / doctor / status / demo)
│   ├── server.rs         axum web server + embedded Lit UI (used by `dbpylot serve`)
│   ├── app.rs            wiring: build the engine from settings + the secret vault
│   ├── opendbpylot.rs    orchestrator: train() + ask() + the self-repair loop
│   ├── prompt.rs         builds the SQL prompt within a token budget
│   ├── schema.rs         pre-execution schema validation (sqlparser)
│   ├── retrieval.rs      hybrid BM25 + vector ranking
│   ├── sql.rs            extract_sql + read-only gate
│   ├── llm.rs / llm/     LlmService trait + openai / anthropic / ollama / retry
│   ├── embedding.rs / …  EmbeddingService trait + local / openai / cache / fastembed
│   ├── vectorstore.rs /… VectorStore trait + memory / file / qdrant
│   ├── sqlrunner.rs / …  SqlRunner trait + sqlite / postgres / mysql / duckdb
│   ├── conversation.rs   ConversationStore (memory + persistent)
│   ├── secret.rs         encrypted secret vault
│   └── settings.rs       user settings model
├── frontends/            TypeScript + Lit web component (compiled into the binary)
└── Cargo.toml
```

---

## Testing

```bash
cargo test                                    # unit tests (repair, schema, retrieval, retry, …)
cargo test --test backends                    # cross-backend integration (SQLite; +DuckDB with the feature)
cargo run --example eval                      # end-to-end accuracy eval (needs an OpenAI key)
```

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
