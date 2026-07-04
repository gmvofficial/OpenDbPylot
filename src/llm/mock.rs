//! Mock LLMs that return canned responses — no network, no API key.
//!
//! - [`MockLlm`] always returns the same response (a stubbed provider for offline use).
//! - [`ScriptedMockLlm`] returns a sequence of responses, one per call — useful for
//!   testing multi-step flows like `intermediate_sql`.

use std::collections::VecDeque;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;

use super::{LlmResponse, LlmService, Message, ToolCall, ToolSchema};

pub struct MockLlm {
    /// Canned text reply (used by the legacy `submit_prompt` path).
    response: String,
    /// Canned SQL the mock "calls" run_sql with when tools are available (so the
    /// offline demo still produces a real table through the agent loop).
    sql: Option<String>,
}

impl MockLlm {
    pub fn new(response: impl Into<String>) -> Self {
        Self { response: response.into(), sql: None }
    }

    /// A canned response that returns a chart-friendly query for the demo
    /// e-commerce database (revenue by product category).
    pub fn with_default_sql() -> Self {
        let sql = "SELECT p.category, ROUND(SUM(oi.quantity * oi.unit_price), 2) AS revenue \
                   FROM order_items oi \
                   JOIN products p ON p.id = oi.product_id \
                   JOIN orders o ON o.id = oi.order_id \
                   WHERE o.status NOT IN ('cancelled','refunded') \
                   GROUP BY p.category ORDER BY revenue DESC";
        Self {
            response: format!("```sql\n{sql};\n```"),
            sql: Some(sql.to_string()),
        }
    }
}

#[async_trait]
impl LlmService for MockLlm {
    async fn submit_prompt(&self, _messages: Vec<Message>) -> Result<String> {
        Ok(self.response.clone())
    }

    /// Drives the agent loop offline so the demo works with no API key:
    ///   1. call `run_sql` with the canned SQL,
    ///   2. then call `visualize_data` on the result file (to show a chart),
    ///   3. then return a final text answer.
    async fn chat(&self, messages: Vec<Message>, tools: &[ToolSchema]) -> Result<LlmResponse> {
        let run_sql_available = tools.iter().any(|t| t.name == "run_sql");
        let viz_available = tools.iter().any(|t| t.name == "visualize_data");
        let has_tool_result = messages.iter().any(|m| m.role == "tool");
        // The visualize_data result (success or failure) always mentions "chart".
        let viz_attempted = messages.iter().any(|m| m.role == "tool" && m.content.contains("chart"));

        // Step 1: run the query.
        if run_sql_available && !has_tool_result {
            if let Some(sql) = &self.sql {
                return Ok(LlmResponse {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "mock-run-sql".into(),
                        name: "run_sql".into(),
                        args: serde_json::json!({ "sql": sql }),
                    }],
                });
            }
        }

        // Step 2: chart the result file, if we have one and haven't yet.
        if viz_available && has_tool_result && !viz_attempted {
            if let Some(filename) = find_results_filename(&messages) {
                return Ok(LlmResponse {
                    text: None,
                    tool_calls: vec![ToolCall {
                        id: "mock-visualize".into(),
                        name: "visualize_data".into(),
                        args: serde_json::json!({ "filename": filename, "title": "Results" }),
                    }],
                });
            }
        }

        // Step 3: final answer.
        if has_tool_result {
            return Ok(LlmResponse::text("Here are the results from your database."));
        }
        Ok(LlmResponse::text(self.response.clone()))
    }
}

/// Find a `query_results_*.json` filename inside the most recent tool message
/// (the one written by run_sql), so the mock can pass it to visualize_data.
fn find_results_filename(messages: &[Message]) -> Option<String> {
    for m in messages.iter().rev() {
        if m.role == "tool" {
            for tok in m.content.split_whitespace() {
                let t = tok.trim_matches(|c| c == '*' || c == '.' || c == ':');
                if t.starts_with("query_results_") && t.ends_with(".json") {
                    return Some(t.to_string());
                }
            }
        }
    }
    None
}

/// Returns a pre-set sequence of responses, one per `submit_prompt` call.
/// After the queue empties it repeats the last response.
pub struct ScriptedMockLlm {
    responses: Mutex<VecDeque<String>>,
    last: Mutex<String>,
}

impl ScriptedMockLlm {
    pub fn new(responses: Vec<String>) -> Self {
        let last = responses.last().cloned().unwrap_or_default();
        Self {
            responses: Mutex::new(responses.into()),
            last: Mutex::new(last),
        }
    }
}

#[async_trait]
impl LlmService for ScriptedMockLlm {
    async fn submit_prompt(&self, _messages: Vec<Message>) -> Result<String> {
        let mut queue = self.responses.lock().unwrap();
        match queue.pop_front() {
            Some(r) => {
                *self.last.lock().unwrap() = r.clone();
                Ok(r)
            }
            None => Ok(self.last.lock().unwrap().clone()),
        }
    }
}

/// A tool-aware mock for testing the agent loop: returns a pre-set sequence of
/// `LlmResponse`s (each may carry text and/or tool calls), one per `chat` call.
/// After the queue empties it repeats the last response.
pub struct ScriptedToolLlm {
    responses: Mutex<VecDeque<LlmResponse>>,
    last: Mutex<LlmResponse>,
}

impl ScriptedToolLlm {
    pub fn new(responses: Vec<LlmResponse>) -> Self {
        let last = responses.last().cloned().unwrap_or_default();
        Self {
            responses: Mutex::new(responses.into()),
            last: Mutex::new(last),
        }
    }

    fn next(&self) -> LlmResponse {
        let mut queue = self.responses.lock().unwrap();
        match queue.pop_front() {
            Some(r) => {
                *self.last.lock().unwrap() = r.clone();
                r
            }
            None => self.last.lock().unwrap().clone(),
        }
    }
}

#[async_trait]
impl LlmService for ScriptedToolLlm {
    async fn submit_prompt(&self, _messages: Vec<Message>) -> Result<String> {
        Ok(self.next().text.unwrap_or_default())
    }

    async fn chat(&self, _messages: Vec<Message>, _tools: &[ToolSchema]) -> Result<LlmResponse> {
        Ok(self.next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{ToolCall, ToolSchema};
    use serde_json::json;

    /// Phase 1 acceptance: a tool-aware LLM returns a parsed tool call, then a
    /// final text answer — the exact shape the agent loop will consume.
    #[tokio::test]
    async fn scripted_tool_llm_returns_tool_call_then_text() {
        let llm = ScriptedToolLlm::new(vec![
            LlmResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "call-1".into(),
                    name: "calculator".into(),
                    args: json!({ "a": 2, "b": 2 }),
                }],
            },
            LlmResponse::text("The answer is 4."),
        ]);

        let tools = vec![ToolSchema {
            name: "calculator".into(),
            description: "adds two numbers".into(),
            parameters: json!({
                "type": "object",
                "properties": { "a": {"type":"number"}, "b": {"type":"number"} },
                "required": ["a", "b"]
            }),
        }];

        // First turn: the model asks to call the tool.
        let r1 = llm.chat(vec![Message::user("what is 2+2?")], &tools).await.unwrap();
        assert!(r1.is_tool_call());
        assert_eq!(r1.tool_calls.len(), 1);
        assert_eq!(r1.tool_calls[0].name, "calculator");
        assert_eq!(r1.tool_calls[0].args["a"], 2);
        assert_eq!(r1.tool_calls[0].args["b"], 2);

        // Second turn (after we'd feed the tool result): plain text answer.
        let r2 = llm.chat(vec![], &tools).await.unwrap();
        assert!(!r2.is_tool_call());
        assert_eq!(r2.text.as_deref(), Some("The answer is 4."));
    }
}
