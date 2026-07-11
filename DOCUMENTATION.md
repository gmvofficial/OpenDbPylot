# OpenDbPylot — Project Documentation

**Natural-language → SQL → answers, in Rust.**

OpenDbPylot turns a plain-English question into SQL, runs it against your database, and
returns the rows. It uses **Retrieval-Augmented Generation (RAG)**: it "learns" your
database from training material (schema, docs, example question→SQL pairs) and retrieves
the most relevant pieces to help an LLM write accurate SQL.

This document is a complete, self-contained guide to the project: what it is, how it is
built, how to run it, and how to extend it.

---

## 1. Table of contents

1. [Overview](#2-overview)
2. [Feature summary](#3-feature-summary)
3. [Getting started](#4-getting-started)
4. [How it works — the RAG pipeline](#5-how-it-works--the-rag-pipeline)
5. [Architecture](#6-architecture)
6. [Project layout](#7-project-layout)
7. [The command-line app](#8-the-command-line-app)
8. [The web app and HTTP API](#9-the-web-app-and-http-api)
9. [Frontends](#10-frontends)
10. [Configuration & secrets](#11-configuration--secrets)
11. [Training your own data](#12-training-your-own-data)
12. [Cargo features & optional backends](#13-cargo-features--optional-backends)
13. [Building, testing & development](#14-building-testing--development)
14. [Extending OpenDbPylot](#15-extending-opendbpylot)
15. [License](#16-license)

---

## 2. Overview

OpenDbPylot is a compact, readable implementation of a "text-to-SQL" assistant. The goal is
to be small enough to read end-to-end while still demonstrating a production-shaped design:
every layer of the pipeline is a Rust **trait**, so concrete providers (OpenAI vs. Ollama,
SQLite vs. Postgres, in-memory vs. hosted vector store) can be swapped without touching the
orchestration logic.

Installed with `cargo install opendbpylot`, it provides a single **`dbpylot`** command: a
setup wizard, a terminal chat REPL, one-shot queries, and an embedded web UI — the frontend
and backend ship together in one binary. There is no fake "demo" state on the web app: it
stays clearly *not connected* until you configure a real LLM provider and a database.

- **Language:** Rust (edition 2021), async via **Tokio**.
- **Interfaces:** the `dbpylot` CLI (chat REPL, `init`, `ask`, `doctor`, `status`) and an
  **Axum** web server with a chat UI and a JSON/SSE/WebSocket API.
- **Core crate name:** `opendbpylot`. **Binary:** `dbpylot`.

---

## 3. Feature summary

- **Natural-language → SQL** through a retrieval-augmented pipeline.
- **Self-repair loop** — generated SQL is validated against the real schema and, on any
  validation issue or execution error, fed back to the model for correction (bounded).
- **Hybrid retrieval** — BM25 keyword scoring fused with embedding cosine similarity (RRF).
- **Swappable providers** for every layer (LLM, embeddings, vector store, SQL runner,
  conversation store) behind traits.
- **Multiple databases:** SQLite, PostgreSQL, MySQL, and **DuckDB** (query CSV/Parquet files
  directly; enable with `--features duckdb`).
- **Multiple LLM backends:** OpenAI, Anthropic, and Ollama (fully local, no key).
- **Persistent training** via a file-backed vector store, plus **auto-import of the live
  database schema** on connect (re-import manually after the structure changes).
- **Multi-turn conversations** that persist, remember history, and can be deleted.
- **Streaming responses** over Server-Sent Events, rendered live in the web UI.
- **`intermediate_sql`** — optionally let the model peek at data before writing the final
  query (opt-in via `allow_llm_to_see_data`).
- **Encrypted secret vault** for API keys and DB passwords (AES-256-GCM with an
  Argon2id-derived key), with an optional OS-keychain fallback.
- **Robustness:** request timeouts, retry-with-backoff on transient LLM failures, a prompt
  token budget, and `OPENDBPYLOT_LOG` tracing.

---

## 4. Getting started

### Prerequisites

- A recent stable **Rust** toolchain (`cargo`).
- **Node.js + npm** only if you want to rebuild the web component (a prebuilt bundle ships
  in the crate).
- An API key from **OpenAI** or **Anthropic**, or a local **Ollama** install (no key).

### Install and set up

```bash
cargo install opendbpylot     # installs the `dbpylot` command

dbpylot init                  # wizard: provider + key (stored encrypted) + database
dbpylot                       # chat with your database
dbpylot ask "how many orders per country?"   # one-shot
```

### From source

```bash
git clone https://github.com/gmvofficial/OpenDbPylot && cd OpenDbPylot
cargo run -- init             # set up
cargo run                     # chat REPL
cargo run -- serve            # web UI at http://127.0.0.1:8080 (opens a browser)
cargo run -- demo             # offline demo on a seeded sample DB (no key)
```

### One-command web launch (from source)

```bash
./start.sh               # frees port 8080, builds the frontend, runs the server, opens the browser
```

---

## 5. How it works — the RAG pipeline

```text
question
  → retrieve relevant context   (similar Q/SQL, related DDL, related docs)   [vectorstore]
  → build a prompt from context                                              [prompt]
  → ask the LLM to write SQL                                                 [llm]
  → extract clean SQL from the reply                                         [sql]
  → run it on the database                                                   [sqlrunner]
  → return rows
```

You first **train** the model with three kinds of material:

```rust
opendbpylot.train_ddl(
    "CREATE TABLE users (id INTEGER, name TEXT, country TEXT, created_at TEXT);"
).await?;

opendbpylot.train_documentation(
    "'country' is the user's country name."
).await?;

opendbpylot.train_question_sql(
    "How many users from each country?",
    "SELECT country, COUNT(*) FROM users GROUP BY country;"
).await?;
```

Then **ask**:

```rust
let answer = opendbpylot.ask("How many users are there per country?").await?;
println!("{}", answer.sql);
```

Training material is embedded and stored in the vector store. At question time the most
similar DDL, docs, and question→SQL examples are retrieved and assembled into a prompt, the
LLM produces a reply, the SQL is extracted and validated, and it is executed against the
database.

---

## 6. Architecture

Every layer is a Rust **trait**, so providers are pluggable:

| Layer          | Trait               | Implementations included                                                                 |
|----------------|---------------------|-------------------------------------------------------------------------------------------|
| LLM            | `LlmService`        | `OpenAiLlm`, `AnthropicLlm`, `OllamaLlm`, `MockLlm`                                       |
| Embeddings     | `EmbeddingService`  | `OpenAiEmbedding`, `LocalEmbedding`                                                       |
| Vector store   | `VectorStore`       | `MemoryVectorStore`, `FileVectorStore` (persistent), `QdrantVectorStore` (`--features qdrant`) |
| SQL runner     | `SqlRunner`         | `SqliteRunner` (plus remote Postgres/MySQL via the `remote-db` feature)                   |
| Conversations  | `ConversationStore` | `MemoryConversationStore` (multi-turn follow-ups)                                        |

The **`OpenDbPylot` struct** (in [`src/opendbpylot.rs`](src/opendbpylot.rs)) is the central
orchestrator that wires these together and exposes `train_*()` and `ask()`.

The crate root [`src/lib.rs`](src/lib.rs) declares the module map: `llm`, `core`,
`capabilities`, `tools`, `embedding`, `conversation`, `vectorstore`, `sqlrunner`, `prompt`,
`sql`, `types`, `opendbpylot`, `demo`, `secret`, `settings`, and `app`.

---

## 7. Project layout

```
opendbpylot/
├── src/
│   ├── lib.rs                crate root / module list
│   ├── server.rs             Axum web server + embedded UI (used by `dbpylot serve`)
│   ├── opendbpylot.rs        orchestrator: train() + ask() + self-repair loop
│   ├── app.rs                shared app/engine wiring (settings + vault → engine)
│   ├── prompt.rs             builds the SQL prompt within a token budget
│   ├── schema.rs             pre-execution schema validation (sqlparser)
│   ├── retrieval.rs          hybrid BM25 + vector ranking (RRF)
│   ├── sql.rs                extract_sql + read-only gate
│   ├── types.rs              shared types
│   ├── demo.rs               shared demo setup (seed DB + training)
│   ├── settings.rs           runtime settings model
│   ├── secret.rs             encrypted secret vault (+ optional keychain)
│   ├── conversation.rs       ConversationStore trait + memory/file impls
│   ├── llm.rs        / llm/         LlmService trait + openai/anthropic/ollama/retry
│   ├── embedding.rs  / embedding/   EmbeddingService trait + local/openai/cache/fastembed
│   ├── vectorstore.rs/ vectorstore/ VectorStore trait + memory/file/qdrant
│   ├── sqlrunner.rs  / sqlrunner/   SqlRunner trait + sqlite/postgres/mysql/duckdb
│   ├── capabilities.rs / capabilities/  capability declarations
│   ├── tools.rs      / tools/       tool definitions (run_sql, visualize_data, memory)
│   ├── core.rs       / core/        the agent tool-loop + RAG enhancer
│   └── bin/
│       ├── dbpylot.rs        the `dbpylot` CLI (init / chat / ask / serve / doctor / status / demo)
│       └── gen_demo.rs       (dev-only, feature-gated) regenerates the demo SQLite DB
├── frontends/
│   ├── src/                  TypeScript + Lit web component source
│   ├── dist/                 prebuilt web-component bundle (served by the server)
│   ├── package.json          npm build config (vite)
│   └── vite.config.ts
├── start.sh                  build frontend + run server + open browser
├── demo.db                   seeded demo SQLite database
├── Cargo.toml                crate manifest, dependencies, and features
└── DOCUMENTATION.md          this file
```

> Note: internal planning notes (`docs/`) and reference material (`extra_repo/`) are kept
> out of version control via `.gitignore`; this document is the canonical project doc.

---

## 8. The command-line app

Installing the crate provides one command, `dbpylot`, with subcommands:

| Command | Does |
|---|---|
| `dbpylot` | interactive chat REPL against your configured database (default) |
| `dbpylot init` | setup wizard — provider, API key (encrypted), database, test connection |
| `dbpylot ask "…"` | answer one question and exit |
| `dbpylot serve [--headless]` | launch the web UI (opens a browser unless `--headless`) |
| `dbpylot doctor` | test the configured LLM + database are reachable |
| `dbpylot status` | print the current configuration (secrets masked) |
| `dbpylot demo` | offline demo on a seeded sample database (no setup needed) |

The chat REPL has line editing, history (↑/↓), colored output, and an animated "thinking"
indicator. Inside it you can also run commands:

```
  │ ❯ How many users are there per country?

  SQL
  SELECT country, COUNT(*) AS user_count FROM users GROUP BY country ORDER BY user_count DESC;

  RESULT
  country  user_count
  ───────  ──────────
  USA      2
  UK       1
```

| REPL command | Does |
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

From source, prefix with `cargo run --`, e.g. `cargo run -- ask "how many users in total?"`.

---

## 9. The web app and HTTP API

Run the server:

```bash
cargo run -- serve        # http://127.0.0.1:8080
```

From the sidebar you can:

- **Settings** — pick an LLM (OpenAI / Anthropic / Ollama / Mock), paste an API key (stored
  in the secret vault), set the SQLite database path, and **Save & connect** (rebuilds the
  engine live).
- **Train** — add documentation / DDL / question→SQL examples, or **Learn schema**.
- **Conversations** — multiple chats that persist and remember history ("New chat", switch
  between them).

### HTTP endpoints (from [`src/server.rs`](src/server.rs))

| Method | Path | Purpose |
|---|---|---|
| `GET`  | `/` | rich web-component chat UI |
| `GET`  | `/opendbpylot-components.js` | the built web-component bundle |
| `GET`  | `/api/providers` | list available LLM providers |
| `GET`/`POST` | `/api/settings` | read / update runtime settings |
| `POST` | `/api/train` | add training material |
| `POST` | `/api/learn_schema` | auto-train from the live DB schema |
| `GET`/`POST` | `/api/conversations` | list / create conversations |
| `GET`  | `/api/conversations/:id` | fetch one conversation |
| `GET`  | `/api/opendbpylot/v2/starter` | starter/suggested questions |
| `POST` | `/api/opendbpylot/v2/chat_sse` | chat over Server-Sent Events (streaming) |
| `POST` | `/api/opendbpylot/v2/chat_poll` | chat via polling |
| `GET`  | `/api/opendbpylot/v2/chat_websocket` | chat over WebSocket |

The streaming flow emits ordered events: `status → sql → result → done`, rendered live in
the UI.

---

## 10. Frontend

The UI is a TypeScript + **Lit** `<opendbpylot-chat>` custom element (served at `/`) that
streams **rich UI components** (`{rich, simple}` chunks) over SSE through a component
registry/manager, rendering live SQL, tables, and **Plotly charts**.

The compiled bundle (`frontends/dist/opendbpylot-components.js`) is **embedded in the binary
at build time** — so `dbpylot serve` is a single self-contained frontend+backend with nothing
to deploy separately. To rebuild it from source (the prebuilt bundle ships in the crate):

```bash
cd frontends && npm install && npm run build
```

Embed it in your own page:

```html
<opendbpylot-chat sse-endpoint="/api/opendbpylot/v2/chat_sse" theme="dark"></opendbpylot-chat>
```

Web-component source lives in `frontends/src/`:
`components/` (chat, message, status bar, progress tracker, rich cards, Plotly chart, task
list), `services/api-client.ts`, and `styles/` (design tokens + component styles).

---

## 11. Configuration & secrets

### Where configuration lives

- **Settings** (non-secret) → `$OPENDBPYLOT_HOME/settings.json` (default `~/.opendbpylot/`).
  Written by `dbpylot init` and the web Settings panel; both share the same directory.
- **Secrets** (API keys, DB connection strings) → the encrypted vault (see below). This is
  the *only* place keys are configured for normal use — run `dbpylot init` to store yours.
- `OPENDBPYLOT_LOG=debug` traces the retrieval → SQL → execution pipeline.
- Dev-only: the offline `dbpylot demo` (and the `eval` example) will use a real LLM if
  `OPENAI_API_KEY` is present in the environment; otherwise the demo runs on the mock.

### Secret vault

API keys are stored in an **encrypted vault** rather than plaintext:

- Default: an AES-256-GCM encrypted file, with the key derived by **Argon2id** from the
  machine ID (no keychain prompts). Set `OPENDBPYLOT_SECRETS=file` to force the `0600`
  file backend.
- Optional: the OS keychain via the `keychain` Cargo feature.

See [`src/secret.rs`](src/secret.rs).

---

## 12. Training your own data

You can bring your own database and teach the model about it in three ways:

1. **DDL** — `CREATE TABLE` statements so the model knows your schema.
2. **Documentation** — business notes describing columns, metrics, and conventions.
3. **Question → SQL examples** — the strongest signal; a few good pairs go a long way.

You can do this from the CLI (`/train ddl|doc|sql`), the web **Train** panel, the
`POST /api/train` endpoint, or programmatically via `train_ddl` / `train_documentation` /
`train_question_sql`. **Learn schema** auto-ingests DDL directly from the connected
database. With a `FileVectorStore`, training persists across runs.

---

## 13. Cargo features & optional backends

Defined in [`Cargo.toml`](Cargo.toml):

| Feature | Default | Enables |
|---|---|---|
| `remote-db` | ✅ | PostgreSQL + MySQL SQL runners via `sqlx` |
| `qdrant` | — | `QdrantVectorStore` (hosted vector DB) via `qdrant-client` |
| `keychain` | — | OS-keychain secret backend via `keyring` |

Enable optional features at build/run time, e.g.:

```bash
cargo run --features qdrant
cargo build --features "qdrant keychain"
```

---

## 14. Building, testing & development

```bash
cargo build                                # build (default features)
cargo run                                  # chat REPL (from source)
cargo run -- serve                         # web server
cargo run --features gen-demo --bin gen_demo   # regenerate the demo SQLite database
cargo test                                 # unit tests
cargo test --test backends                 # cross-backend integration tests
cargo run --example eval                   # end-to-end accuracy eval (needs an OpenAI key)
```

Postgres/MySQL integration tests are skipped unless `OPENDBPYLOT_TEST_POSTGRES_URL` /
`OPENDBPYLOT_TEST_MYSQL_URL` point at live servers; DuckDB tests need `--features duckdb`.

Binaries:

- **`dbpylot`** — the single user-facing command (`src/bin/dbpylot.rs`).
- `gen_demo` — dev-only, feature-gated (`--features gen-demo`); regenerates `demo.db`.

---

## 15. Extending OpenDbPylot

Because every layer is a trait, extending the system means implementing one trait and
plugging it into the `OpenDbPylot` orchestrator:

- **New LLM provider** → implement `LlmService`.
- **New embeddings** → implement `EmbeddingService`.
- **New vector store** → implement `VectorStore`.
- **New database** → implement `SqlRunner`.
- **New conversation backend** → implement `ConversationStore`.

Keep the orchestration in `src/opendbpylot.rs` untouched; only the concrete implementation
changes. This "interface + plug-in" design is the core idea of the codebase.

---

## 16. License

Licensed under the Apache License, Version 2.0. See [LICENSE](LICENSE).
