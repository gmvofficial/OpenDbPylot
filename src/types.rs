//! Small shared data types used across modules.

use serde::{Deserialize, Serialize};

/// A question paired with the SQL that answers it.
///
/// This is the unit of "self-learning" in OpenDbPylot: successful question/SQL pairs are
/// stored and later retrieved as few-shot examples for new, similar questions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionSql {
    pub question: String,
    pub sql: String,
}
