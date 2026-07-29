# opendbpylot (Node.js)

Node.js / TypeScript bindings for [**opendbpylot**](https://github.com/gmvofficial/OpenDbPylot) —
turn a natural-language question into SQL, run it on your database, and get the results.
Uses Retrieval-Augmented Generation (RAG) with a self-repairing SQL loop.

```bash
npm install opendbpylot
```

Prebuilt native addons ship for macOS (x64/arm64), Linux (x64/arm64, glibc), and
Windows (x64) — no compiler needed.

This installs both a Node library **and** a fully-working `dbpylot` command — the
whole CLI runs in-process from the native addon, so **no Rust toolchain is needed**:

```bash
npm install -g opendbpylot
dbpylot init        # setup wizard
dbpylot             # chat with your database
dbpylot serve       # web UI
dbpylot ask "how many orders per country?"
dbpylot mcp         # serve as an MCP server for agent hosts (OpenPylot, Claude Desktop, …)
```

`dbpylot mcp` exposes the engine over the Model Context Protocol (stdio). Secrets can be set
non-interactively for scripted setups: `printf '%s' "$KEY" | dbpylot config set-key openai`
and `dbpylot config set-db sqlite /data/app.db`.

## Usage (library)

First configure an LLM provider + a database. The bindings share the same config as the
`dbpylot` CLI (install it with `cargo install opendbpylot`), so run the wizard once:

```bash
dbpylot init
```

Then:

```ts
import { OpenDbPylot } from "opendbpylot";

const bot = new OpenDbPylot();
const result = bot.ask("how many orders per country?");

console.log(result.sql);        // the generated (and self-repaired) SQL
console.log(result.columns);    // column names
for (const row of result.rows)  // result rows
  console.log(row);

// Teach it about your schema / business rules:
bot.trainDocumentation("Revenue excludes cancelled and refunded orders.");
bot.trainQuestionSql("top products", "SELECT name FROM products ORDER BY price DESC LIMIT 10;");
```

## Fully self-contained web UI

You don't even need the CLI — launch the embedded web app straight from Node and
configure everything (LLM + database) in the browser Settings panel:

```ts
import { OpenDbPylot } from "opendbpylot";
OpenDbPylot.serve();   // opens http://127.0.0.1:8080, runs until interrupted
```

## API

- `new OpenDbPylot()` — load the engine from your saved configuration.
- `.ask(question) → AskResult` — `{ sql, columns, rows, repairsUsed, answer? }`.
- `.trainDdl(ddl)`, `.trainDocumentation(doc)`, `.trainQuestionSql(q, sql)`.
- `OpenDbPylot.serve()` — run the embedded web UI **in-process** (no CLI binary needed).
- `OpenDbPylot.init()` / `.doctor()` — convenience shims that call the `dbpylot` CLI
  (`cargo install opendbpylot`); `serve()` + Settings does the same setup.

Licensed under Apache-2.0.
