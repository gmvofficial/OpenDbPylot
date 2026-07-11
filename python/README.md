# opendbpylot (Python)

Python bindings for [**opendbpylot**](https://github.com/gmvofficial/OpenDbPylot) —
turn a natural-language question into SQL, run it on your database, and get the results.
Uses Retrieval-Augmented Generation (RAG) with a self-repairing SQL loop.

```bash
pip install opendbpylot
```

This installs both a Python library **and** a fully-working `dbpylot` command — the
whole CLI runs in-process from the native module, so **no Rust toolchain is needed**:

```bash
dbpylot init        # setup wizard
dbpylot             # chat with your database
dbpylot serve       # web UI
dbpylot ask "how many orders per country?"
```

## Usage (library)

First configure an LLM provider + a database. The bindings share the same config as the
`dbpylot` CLI (install it with `cargo install opendbpylot`), so run the wizard once:

```bash
dbpylot init
# …or, from Python:
python -c "import opendbpylot; opendbpylot.OpenDbPylot.init()"
```

Then:

```python
import opendbpylot

bot = opendbpylot.OpenDbPylot()
result = bot.ask("how many orders per country?")

print(result["sql"])          # the generated (and self-repaired) SQL
print(result["columns"])      # column names
for row in result["rows"]:    # result rows
    print(row)

# Teach it about your schema / business rules:
bot.train_documentation("Revenue excludes cancelled and refunded orders.")
bot.train_question_sql("top products", "SELECT name FROM products ORDER BY price DESC LIMIT 10;")
```

## Fully self-contained web UI

You don't even need the CLI — launch the embedded web app straight from Python and
configure everything (LLM + database) in the browser Settings panel:

```python
import opendbpylot
opendbpylot.OpenDbPylot.serve()   # opens http://127.0.0.1:8080, runs until interrupted
```

## API

- `OpenDbPylot()` — load the engine from your saved configuration.
- `.ask(question) -> dict` — `{sql, columns, rows, repairs_used, answer?}`.
- `.train_ddl(ddl)`, `.train_documentation(doc)`, `.train_question_sql(q, sql)`.
- `OpenDbPylot.serve()` — run the embedded web UI **in-process** (no CLI binary needed).
- `OpenDbPylot.init()` / `.doctor()` — convenience shims that call the `dbpylot` CLI
  (install it with `cargo install opendbpylot`); `serve()` + Settings does the same setup.

Licensed under Apache-2.0.
