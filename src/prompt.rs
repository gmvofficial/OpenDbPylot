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

/// Rough token budget, a `max_tokens`-style budget. We approximate ~4 chars/token.
const MAX_TOKENS: usize = 14_000;

fn approx_tokens(s: &str) -> usize {
    s.len() / 4
}

pub fn build_sql_prompt(
    dialect: &str,
    question: &str,
    ddl_list: &[String],
    doc_list: &[String],
    question_sql_list: &[QuestionSql],
    history: &[QuestionSql],
) -> Vec<Message> {
    let mut initial = format!(
        "You are a {dialect} expert. Please help to generate a SQL query to answer the \
         question. Your response should ONLY be based on the given context and follow the \
         response guidelines and format instructions. "
    );

    // ===Tables (DDL)
    if !ddl_list.is_empty() {
        initial.push_str("\n===Tables \n");
        for ddl in ddl_list {
            if approx_tokens(&initial) + approx_tokens(ddl) < MAX_TOKENS {
                initial.push_str(ddl);
                initial.push_str("\n\n");
            }
        }
    }

    // ===Additional Context (documentation)
    if !doc_list.is_empty() {
        initial.push_str("\n===Additional Context \n\n");
        for doc in doc_list {
            if approx_tokens(&initial) + approx_tokens(doc) < MAX_TOKENS {
                initial.push_str(doc);
                initial.push_str("\n\n");
            }
        }
    }

    // ===Response Guidelines (verbatim from OpenDbPylot)
    initial.push_str(&format!(
        "===Response Guidelines \n\
         1. If the provided context is sufficient, please generate a valid SQL query without any explanations for the question. \n\
         2. If the provided context is almost sufficient but requires knowledge of a specific string in a particular column, please generate an intermediate SQL query to find the distinct strings in that column. Prepend the query with a comment saying intermediate_sql \n\
         3. If the provided context is insufficient, please explain why it can't be generated. \n\
         4. Please use the most relevant table(s). \n\
         5. If the question has been asked and answered before, please repeat the answer exactly as it was given before. \n\
         6. Ensure that the output SQL is {dialect}-compliant and executable, and free of syntax errors. \n"
    ));

    // Few-shot: each retrieved example becomes a user turn, its SQL an assistant turn.
    let mut messages = vec![Message::system(initial)];
    for example in question_sql_list {
        messages.push(Message::user(example.question.clone()));
        messages.push(Message::assistant(example.sql.clone()));
    }

    // Conversation history (actual prior turns) goes closest to the new question,
    // so follow-ups like "and only from the USA?" have context.
    for turn in history {
        messages.push(Message::user(turn.question.clone()));
        messages.push(Message::assistant(turn.sql.clone()));
    }

    // Finally, the real question.
    messages.push(Message::user(question.to_string()));
    messages
}
