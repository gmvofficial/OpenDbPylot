//! OpenAI implementation of the `LlmService` trait.
//!
//! This is the "concrete plug-in" — like `src/opendbpylot/integrations/openai/`
//! in the Python project. It knows the exact HTTP shape OpenAI expects, including
//! the `tools` / `tool_calls` function-calling format used by the agent loop.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{LlmResponse, LlmService, Message, ToolCall, ToolSchema};

/// Holds the config + a reusable HTTP client.
pub struct OpenAiLlm {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenAiLlm {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

// --- The shape of OpenAI's JSON response (only the bits we care about) ---

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<RespToolCall>,
}

#[derive(Deserialize)]
struct RespToolCall {
    id: String,
    function: RespFunction,
}

#[derive(Deserialize)]
struct RespFunction {
    name: String,
    /// OpenAI returns arguments as a JSON-encoded **string**.
    arguments: String,
}

/// Translate the neutral messages into OpenAI's message array.
///
/// - assistant turns with tool calls → `{role, content, tool_calls:[{id,type:function,
///   function:{name, arguments}}]}` (arguments as a JSON string).
/// - tool turns → `{role:"tool", tool_call_id, content}`.
/// - everything else → `{role, content}`.
fn build_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            if m.role == "assistant" && !m.tool_calls.is_empty() {
                let calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|tc| json!({
                        "id": tc.id,
                        "type": "function",
                        "function": {
                            "name": tc.name,
                            "arguments": tc.args.to_string(),
                        },
                    }))
                    .collect();
                json!({
                    "role": "assistant",
                    "content": if m.content.is_empty() { Value::Null } else { json!(m.content) },
                    "tool_calls": calls,
                })
            } else if m.role == "tool" {
                json!({
                    "role": "tool",
                    "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
                    "content": m.content,
                })
            } else {
                json!({ "role": m.role, "content": m.content })
            }
        })
        .collect()
}

/// Render tool schemas into OpenAI's `tools` array (function-calling format).
fn build_tools(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| json!({
            "type": "function",
            "function": {
                "name": t.name,
                "description": t.description,
                "parameters": t.parameters,
            },
        }))
        .collect()
}

#[async_trait]
impl LlmService for OpenAiLlm {
    async fn submit_prompt(&self, messages: Vec<Message>) -> Result<String> {
        let body = json!({
            "model": self.model,
            "messages": build_messages(&messages),
        });
        let data = self.call(body).await?;
        let content = data
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();
        Ok(content)
    }

    async fn chat(&self, messages: Vec<Message>, tools: &[ToolSchema]) -> Result<LlmResponse> {
        let mut body = json!({
            "model": self.model,
            "messages": build_messages(&messages),
        });
        if !tools.is_empty() {
            body["tools"] = json!(build_tools(tools));
            body["tool_choice"] = json!("auto");
        }
        let data = self.call(body).await?;

        let msg = match data.choices.into_iter().next() {
            Some(c) => c.message,
            None => return Ok(LlmResponse::default()),
        };

        let tool_calls = msg
            .tool_calls
            .into_iter()
            .map(|tc| ToolCall {
                id: tc.id,
                name: tc.function.name,
                // arguments is a JSON string; parse it, defaulting to {}.
                args: serde_json::from_str(&tc.function.arguments).unwrap_or_else(|_| json!({})),
            })
            .collect();

        Ok(LlmResponse {
            text: msg.content.filter(|s| !s.is_empty()),
            tool_calls,
        })
    }
}

impl OpenAiLlm {
    async fn call(&self, body: Value) -> Result<ChatResponse> {
        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to send request to OpenAI")?
            .error_for_status()
            .context("OpenAI returned an error status")?;
        response.json().await.context("failed to parse OpenAI response")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assistant_tool_call_and_tool_result_translate_correctly() {
        let messages = vec![
            Message::user("q"),
            Message::assistant_tool_calls(
                "",
                vec![ToolCall { id: "c1".into(), name: "run_sql".into(), args: json!({"sql":"SELECT 1"}) }],
            ),
            Message::tool_result("c1", "1"),
        ];
        let out = build_messages(&messages);
        assert_eq!(out[0], json!({ "role": "user", "content": "q" }));

        // assistant turn carries a function tool_call with arguments as a string.
        assert_eq!(out[1]["role"], "assistant");
        assert_eq!(out[1]["content"], Value::Null);
        assert_eq!(out[1]["tool_calls"][0]["id"], "c1");
        assert_eq!(out[1]["tool_calls"][0]["function"]["name"], "run_sql");
        assert_eq!(out[1]["tool_calls"][0]["function"]["arguments"], "{\"sql\":\"SELECT 1\"}");

        // tool result turn.
        assert_eq!(out[2], json!({ "role": "tool", "tool_call_id": "c1", "content": "1" }));
    }
}
