//! Interactive CLI for opendbpylot — a friendly elephant that turns your questions
//! into SQL and runs them, with a boxed, visual layout.
//!
//! Usage:
//!   cargo run                         # interactive chat (REPL)
//!   cargo run -- "your question"      # one-shot: answer once and exit
//!
//! Runs offline (mock LLM) unless OPENAI_API_KEY is set in your .env.

use std::time::Duration;

use anyhow::Result;
use colored::{Color, Colorize};
use indicatif::{ProgressBar, ProgressStyle};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

use opendbpylot::demo::build_demo_opendbpylot;
use opendbpylot::sqlrunner::QueryResult;
use opendbpylot::opendbpylot::OpenDbPylot;

/// Our mascot. 🐘
const ELEPHANT: &str = r#"
          ___       ___
         (   `.   .'   )
          \    `.'    /
           |  o   o  |
           |    ^    |          v a n n a - r s
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

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let (opendbpylot, backend) = build_demo_opendbpylot().await?;

    // One-shot mode: `cargo run -- "how many users?"`
    let args: Vec<String> = std::env::args().skip(1).collect();
    if !args.is_empty() {
        answer(&opendbpylot, "cli", &args.join(" ")).await;
        return Ok(());
    }

    print_banner(backend);
    repl(&opendbpylot).await
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
            match ans.result {
                Some(rows) => print_box("RESULT", &result_lines(&rows), Color::Cyan, None),
                None => println!(
                    "  {}",
                    "(not run — not a read query or no database)".dimmed()
                ),
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
