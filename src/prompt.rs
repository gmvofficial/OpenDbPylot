//! Builds the SQL-generation prompt.
//!
//! The structure (and the exact response guidelines) match OpenDbPylot so the model
//! behaves the same way:
//!   1. a system message: "You are a {dialect} expert..."
//!   2. the related table definitions   (===Tables)
//!   3. the related documentation       (===Additional Context)
//!   4. numbered response guidelines    (===Response Guidelines)
//!   5. similar question/SQL pairs as few-shot user/assistant turns
//!   6. the user's actual question

use crate::llm::Message;
use crate::types::QuestionSql;

/// Default prompt token budget, used when a caller doesn't specify one.
/// (`OpenDbPylotConfig::max_prompt_tokens` supplies it in the normal path.)
pub const DEFAULT_MAX_PROMPT_TOKENS: usize = 14_000;

/// Rough token estimate: ~4 characters per token. Good enough to keep prompts
/// off the model's hard context limit without pulling in a real tokenizer.
fn approx_tokens(s: &str) -> usize {
    s.len() / 4
}

/// A running token budget for assembling a prompt out of prioritized sections.
///
/// The mandatory skeleton (preamble, guidelines, the question itself) is
/// [`reserve`](Self::reserve)d up front so it can never be squeezed out; optional
/// context (DDL → docs → examples → history) is then added highest-priority-first
/// via [`take`](Self::take), which only spends budget when the item fits. On a
/// huge schema this means the *most relevant* DDL survives and the least relevant
/// context is dropped, instead of blowing the context window.
struct PromptBudget {
    remaining: usize,
}

impl PromptBudget {
    fn new(max_tokens: usize) -> Self {
        Self { remaining: max_tokens }
    }

    /// Unconditionally account for mandatory text (never rejected; saturates at 0).
    fn reserve(&mut self, text: &str) {
        self.remaining = self.remaining.saturating_sub(approx_tokens(text));
    }

    /// Spend `cost` tokens if they fit; returns whether the item was accepted.
    fn take_cost(&mut self, cost: usize) -> bool {
        if cost <= self.remaining {
            self.remaining -= cost;
            true
        } else {
            false
        }
    }

    /// Spend the tokens for `text` if it fits.
    fn take(&mut self, text: &str) -> bool {
        self.take_cost(approx_tokens(text))
    }
}

/// The fixed response guidelines (depends only on the dialect).
fn response_guidelines(dialect: &str) -> String {
    format!(
        "===Response Guidelines \n\
         1. If the provided context is sufficient, please generate a valid SQL query without any explanations for the question. \n\
         2. If the provided context is almost sufficient but requires knowledge of a specific string in a particular column, please generate an intermediate SQL query to find the distinct strings in that column. Prepend the query with a comment saying intermediate_sql \n\
         3. If the provided context is insufficient, please explain why it can't be generated. \n\
         4. Please use the most relevant table(s). \n\
         5. If the question has been asked and answered before, please repeat the answer exactly as it was given before. \n\
         6. Ensure that the output SQL is {dialect}-compliant and executable, and free of syntax errors. \n\
         7. When the question asks for a quantity ('how many', 'how much', 'number of', 'count of'), return the aggregated value using COUNT, COUNT(DISTINCT ...), or SUM — not the list of underlying rows. (A grouped 'per X' question still returns a count per group.) \n"
    )
}

pub fn build_sql_prompt(
    dialect: &str,
    question: &str,
    ddl_list: &[String],
    doc_list: &[String],
    question_sql_list: &[QuestionSql],
    history: &[QuestionSql],
    max_tokens: usize,
) -> Vec<Message> {
    let preamble = format!(
        "You are a {dialect} expert. Please help to generate a SQL query to answer the \
         question. Your response should ONLY be based on the given context and follow the \
         response guidelines and format instructions. "
    );
    let guidelines = response_guidelines(dialect);

    // Reserve the mandatory skeleton first — it must always fit.
    let mut budget = PromptBudget::new(max_tokens);
    budget.reserve(&preamble);
    budget.reserve(&guidelines);
    budget.reserve(question);

    // ===Tables (DDL) — highest-priority context, relevance-ordered by retrieval.
    // Greedy: keep each item that still fits (skipping an oversized one lets a
    // smaller, also-relevant one through).
    let mut tables = String::new();
    for ddl in ddl_list {
        if budget.take(ddl) {
            tables.push_str(ddl);
            tables.push_str("\n\n");
        }
    }

    // ===Additional Context (documentation) — next priority.
    let mut docs = String::new();
    for doc in doc_list {
        if budget.take(doc) {
            docs.push_str(doc);
            docs.push_str("\n\n");
        }
    }

    // Assemble the system message in the canonical order.
    let mut initial = preamble;
    if !tables.is_empty() {
        initial.push_str("\n===Tables \n");
        initial.push_str(&tables);
    }
    if !docs.is_empty() {
        initial.push_str("\n===Additional Context \n\n");
        initial.push_str(&docs);
    }
    initial.push_str(&guidelines);

    let mut messages = vec![Message::system(initial)];

    // Few-shot examples — relevance-ordered; keep the most relevant that fit.
    for example in question_sql_list {
        let cost = approx_tokens(&example.question) + approx_tokens(&example.sql);
        if budget.take_cost(cost) {
            messages.push(Message::user(example.question.clone()));
            messages.push(Message::assistant(example.sql.clone()));
        }
    }

    // Conversation history goes closest to the new question so follow-ups like
    // "and only from the USA?" have context. Under budget pressure keep the MOST
    // RECENT turns (walk newest→oldest, stop at the first that doesn't fit), then
    // emit them back in chronological order.
    let mut kept: Vec<&QuestionSql> = Vec::new();
    for turn in history.iter().rev() {
        let cost = approx_tokens(&turn.question) + approx_tokens(&turn.sql);
        if budget.take_cost(cost) {
            kept.push(turn);
        } else {
            break;
        }
    }
    for turn in kept.iter().rev() {
        messages.push(Message::user(turn.question.clone()));
        messages.push(Message::assistant(turn.sql.clone()));
    }

    // Finally, the real question (its budget was reserved up front).
    messages.push(Message::user(question.to_string()));
    messages
}

/// Build the self-repair prompt: a previously generated query failed (schema
/// validation or execution); show the model the error plus the table definitions
/// and ask for a corrected query — SQL only, no prose.
pub fn build_repair_prompt(
    dialect: &str,
    question: &str,
    failed_sql: &str,
    error: &str,
    ddl_list: &[String],
    max_tokens: usize,
) -> Vec<Message> {
    let preamble = format!(
        "You are a {dialect} expert. A SQL query written to answer the user's question \
         failed. Use the error message and the table definitions below to correct it. \
         Respond with ONLY the corrected {dialect} SQL query — no explanations. "
    );
    let user = format!(
        "Question: {question}\n\nFailed SQL:\n{failed_sql}\n\nError:\n{error}\n\nCorrected SQL:"
    );

    // Reserve the mandatory parts, then fit as much DDL as the budget allows.
    let mut budget = PromptBudget::new(max_tokens);
    budget.reserve(&preamble);
    budget.reserve(&user);

    let mut system = preamble;
    if !ddl_list.is_empty() {
        system.push_str("\n===Tables \n");
        for ddl in ddl_list {
            if budget.take(ddl) {
                system.push_str(ddl);
                system.push_str("\n\n");
            }
        }
    }

    vec![Message::system(system), Message::user(user)]
}

/// Build the result-summary prompt: given the question, the SQL that answered it,
/// and a preview of the result rows, ask for a one/two-sentence plain-English
/// takeaway (not a restatement of the SQL or a dump of the rows).
pub fn build_summary_prompt(question: &str, sql: &str, result_preview: &str) -> Vec<Message> {
    vec![
        Message::system(
            "You are a data analyst. Answer the user's question in ONE or TWO short sentences \
             based on the query result. Do not restate the SQL or list every row — give the \
             takeaway (e.g. \"Revenue peaked in March at $42k, up 12% from February.\").",
        ),
        Message::user(format!(
            "Question: {question}\n\nSQL: {sql}\n\nResult:\n{result_preview}\n\nAnswer:"
        )),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qs(q: &str, sql: &str) -> QuestionSql {
        QuestionSql { question: q.into(), sql: sql.into() }
    }

    fn system_text(messages: &[Message]) -> String {
        messages.iter().find(|m| m.role == "system").unwrap().content.clone()
    }

    #[test]
    fn everything_fits_under_a_generous_budget() {
        let ddl = vec!["CREATE TABLE t (id INT, name TEXT);".to_string()];
        let docs = vec!["t holds things".to_string()];
        let examples = vec![qs("count things", "SELECT COUNT(*) FROM t;")];
        let history = vec![qs("prior", "SELECT 1;")];

        let msgs = build_sql_prompt("SQLite", "how many things?", &ddl, &docs, &examples, &history, 14_000);

        let sys = system_text(&msgs);
        assert!(sys.contains("CREATE TABLE t"));
        assert!(sys.contains("t holds things"));
        // system + (example user+assistant) + (history user+assistant) + question
        assert_eq!(msgs.len(), 1 + 2 + 2 + 1);
        assert_eq!(msgs.last().unwrap().content, "how many things?");
    }

    #[test]
    fn ddl_is_kept_but_low_priority_context_is_dropped_under_pressure() {
        // Budget comfortably covers the skeleton (~330 tok) + a tiny DDL, but not
        // the ~1500-token example/history that come after it.
        let huge = "SELECT ".to_string() + &"x, ".repeat(2000) + "1;";
        let ddl = vec!["CREATE TABLE t (id INT);".to_string()];
        let examples = vec![qs("q", &huge)];
        let history = vec![qs("h", &huge)];

        let msgs = build_sql_prompt("SQLite", "q?", &ddl, &[], &examples, &history, 600);

        let sys = system_text(&msgs);
        assert!(sys.contains("CREATE TABLE t"), "DDL (top priority) must survive");
        // The oversized example + history were dropped; only system + question remain.
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs.last().unwrap().content, "q?");
    }

    #[test]
    fn history_keeps_most_recent_turns() {
        // Two ~1500-token turns, chronological (oldest first). The budget fits the
        // skeleton + exactly one of them, so the newest must win.
        let huge = "SELECT ".to_string() + &"col, ".repeat(500) + "1;";
        let history = vec![qs("oldest", &huge), qs("newest", &huge)];

        // ~330 skeleton + one ~630-tok turn fits; a second would not.
        let msgs = build_sql_prompt("SQLite", "q?", &[], &[], &[], &history, 1250);

        let user_turns: Vec<&str> =
            msgs.iter().filter(|m| m.role == "user").map(|m| m.content.as_str()).collect();
        assert!(user_turns.contains(&"newest"), "should keep the most recent turn: {user_turns:?}");
        assert!(!user_turns.contains(&"oldest"), "should drop the oldest turn: {user_turns:?}");
        assert_eq!(msgs.last().unwrap().content, "q?");
    }

    #[test]
    fn question_and_guidelines_survive_a_tiny_budget() {
        // Even with a budget of 0, the mandatory skeleton is never dropped.
        let ddl = vec!["CREATE TABLE t (id INT);".to_string()];
        let msgs = build_sql_prompt("SQLite", "the question", &ddl, &[], &[], &[], 0);
        let sys = system_text(&msgs);
        assert!(sys.contains("Response Guidelines"));
        assert!(!sys.contains("CREATE TABLE t"), "no budget → no optional DDL");
        assert_eq!(msgs.last().unwrap().content, "the question");
    }

    #[test]
    fn repair_prompt_carries_error_and_question() {
        let msgs = build_repair_prompt(
            "SQLite",
            "how many users?",
            "SELECT nam FROM users;",
            "no such column: nam",
            &["CREATE TABLE users (id INT, name TEXT);".to_string()],
            14_000,
        );
        assert_eq!(msgs.len(), 2);
        assert!(system_text(&msgs).contains("CREATE TABLE users"));
        let user = &msgs[1].content;
        assert!(user.contains("no such column: nam"));
        assert!(user.contains("SELECT nam FROM users;"));
    }
}
