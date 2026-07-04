# 🦀 opendbpylot

**Natural-language → SQL → answers, in Rust.**

**opendbpylot** turns a natural-language question into SQL, runs it on your database, and shows
you the results. It uses **Retrieval-Augmented Generation (RAG)** — it "learns" your database from
training material and retrieves the relevant pieces to help an LLM write accurate SQL.

> A compact, self-contained implementation — small enough to read end-to-end.

📖 **Full project documentation:** [DOCUMENTATION.md](DOCUMENTATION.md)

---

## Quick start

```bash
# 1. (optional) use real OpenAI — otherwise it runs offline with a mock LLM
cp .env.example .env        # then paste your OPENAI_API_KEY

# 2. CLI demo: train a tiny model, ask a question, print SQL + results
cargo run

# 3. Web app: chat UI + JSON API at http://127.0.0.1:8080
cargo run --bin server
```

### Self-serve app (no .env needed)

`cargo run --bin server` → open http://127.0.0.1:8080. From the **sidebar** you can:
- **Settings** — pick an LLM (OpenAI / Anthropic / Ollama / Mock), paste an API key
  (stored in a **secret vault**: OS keychain, or a `0600` file with `OPENDBPYLOT_SECRETS=file`),
  set the SQLite database path, and **Save & connect** (rebuilds the engine live).
- **Train** — add documentation / DDL / question→SQL examples, or **Learn schema**.
- **Conversations** — multiple chats that **persist and remember history**, like a
  normal chat tool ("New chat", switch between them).

It boots in offline **Mock** mode (seeded demo DB), so it works with zero config; add a
real provider + key in Settings to use your own data. See
[docs/APP_PLAN.md](docs/APP_PLAN.md) and [docs/BRING_YOUR_OWN_DATA.md](docs/BRING_YOUR_OWN_DATA.md).

### Web component internals

Two frontends are included:

- **Web component** (served at `/`) — a TypeScript + **Lit** `<opendbpylot-chat>`
  custom element that streams **rich UI components** (`{rich, simple}` chunks) over SSE
  through a component registry/manager, rendering live SQL, tables, and **Plotly charts**.
  Build it once:
  ```bash
  cd frontends/webcomponent && npm install && npm run build
  ```
  Then `cargo run --bin server` and open http://127.0.0.1:8080.
  Embed anywhere: `<opendbpylot-chat sse-endpoint="/api/opendbpylot/v2/chat_sse" theme="dark"></opendbpylot-chat>`.
- **Zero-build page** (served at `/simple`) — a single vanilla-JS HTML page, no Node needed.

See [docs/FRONTEND_PLAN.md](docs/FRONTEND_PLAN.md) for the architecture and parity checklist.

No API key? It automatically falls back to an **offline mock LLM + local embeddings**,
so the whole pipeline still runs (the SQL is canned, but everything else is real).

---

## Interactive CLI 🐘

`cargo run` launches a friendly elephant REPL where you can type questions and run
commands. It has line editing, history (↑/↓), colored output, and an animated
"thinking" elephant while it works.

```
opendbpylot ❯ How many users are there per country?

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

One-shot (scripting): `cargo run -- "how many users in total?"`

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
| SQL runner   | `SqlRunner`        | `SqliteRunner`                                    |
| Conversations| `ConversationStore`| `MemoryConversationStore` (multi-turn follow-ups) |

**v2 features:** persistent training (`FileVectorStore`), auto-training from the live
DB schema (`/schema`), `intermediate_sql` (let the model peek at data, opt-in via
`allow_llm_to_see_data`), and **streaming** responses over SSE
(`GET /api/ask_sse` → `status → sql → result → done`, rendered live in the web UI).

The `OpenDbPylot` struct (in [`src/opendbpylot.rs`](src/opendbpylot.rs)) wires them together — it's the
central orchestrator that the whole pipeline hangs off of.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full map, and
[`docs/CONCEPT.md`](docs/CONCEPT.md) for a beginner explanation of RAG.

---

## Project layout

```
opendbpylot/
├── src/
│   ├── lib.rs              crate root / module list
│   ├── main.rs            CLI demo
│   ├── bin/server.rs      axum web server (API + frontend)
│   ├── opendbpylot.rs           orchestrator: train() + ask()
│   ├── prompt.rs          builds the SQL prompt
│   ├── sql.rs             extract_sql + is_sql_valid
│   ├── types.rs           shared types
│   ├── demo.rs            shared demo setup (db + training)
│   ├── llm.rs / llm/      LlmService trait + mock + openai
│   ├── embedding.rs / …   EmbeddingService trait + local + openai
│   ├── vectorstore.rs / … VectorStore trait + in-memory store
│   └── sqlrunner.rs / …   SqlRunner trait + sqlite
├── frontend/index.html    chat UI (vanilla JS, no build step)
├── docs/                  plan, concept, architecture, roadmap
└── Cargo.toml
```

---

## Testing

```bash
cargo test        # unit tests (SQL extraction, validity)
```

## License

MIT
