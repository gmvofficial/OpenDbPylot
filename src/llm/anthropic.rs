//! Anthropic (Claude) implementation of `LlmService`.
//!
//! Anthropic's API separates the `system` prompt from the `messages` list and only
//! allows `user`/`assistant` roles, so we split our messages accordingly. Tool calls
//! are `tool_use` content blocks on assistant turns, and tool *results* must be
//! `tool_result` blocks grouped inside a single `user` message (see
//! `build_payload` / PROJECT_STUDY §5.7).

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{LlmResponse, LlmService, Message, ToolCall, ToolSchema};

/// Default request timeout (see `openai.rs` — same reasoning).
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

pub struct AnthropicLlm {
    client: reqwest::Client,
    api_key: String,
    model: String,
    max_tokens: u32,
}

impl AnthropicLlm {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: super::http_client(DEFAULT_TIMEOUT),
            api_key: api_key.into(),
            model: model.into(),
            max_tokens: 1024,
        }
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    /// Override the request timeout (from user settings).
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.client = super::http_client(timeout);
        self
    }
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    // tool_use fields
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    input: Option<Value>,
}

/// Split the neutral messages into Anthropic's `(system, messages)` shape.
///
/// - `system`/`user`/`assistant` text → plain string content.
/// - assistant turns with tool calls → content blocks (optional text + `tool_use`).
/// - `tool` turns → `tool_result` blocks, with **consecutive tool turns merged into
///   one `user` message** (Anthropic requires all tool results for a turn together).
fn build_payload(messages: &[Message]) -> (String, Vec<Value>) {
    let mut system = String::new();
    let mut out: Vec<Value> = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let m = &messages[i];
        match m.role.as_str() {
            "system" => {
                if !system.is_empty() {
                    system.push('\n');
                }
                system.push_str(&m.content);
                i += 1;
            }
            "tool" => {
                // Merge this and any following consecutive tool messages.
                let mut blocks = Vec::new();
                while i < messages.len() && messages[i].role == "tool" {
                    let t = &messages[i];
                    blocks.push(json!({
                        "type": "tool_result",
                        "tool_use_id": t.tool_call_id.clone().unwrap_or_default(),
                        "content": t.content,
                    }));
                    i += 1;
                }
                out.push(json!({ "role": "user", "content": blocks }));
            }
            "assistant" if !m.tool_calls.is_empty() => {
                let mut blocks = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(json!({ "type": "text", "text": m.content }));
                }
                for tc in &m.tool_calls {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.args,
                    }));
                }
                out.push(json!({ "role": "assistant", "content": blocks }));
                i += 1;
            }
            role => {
                out.push(json!({ "role": role, "content": m.content }));
                i += 1;
            }
        }
    }
    (system, out)
}

/// Render tool schemas into Anthropic's `tools` array (uses `input_schema`).
fn build_tools(tools: &[ToolSchema]) -> Vec<Value> {
    tools
        .iter()
        .map(|t| json!({
            "name": t.name,
            "description": t.description,
            "input_schema": t.parameters,
        }))
        .collect()
}

#[async_trait]
impl LlmService for AnthropicLlm {
    async fn submit_prompt(&self, messages: Vec<Message>) -> Result<String> {
        let (system, turns) = build_payload(&messages);
        let body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": turns,
        });
        let data = self.call(body).await?;
        Ok(text_of(&data.content))
    }

    async fn chat(&self, messages: Vec<Message>, tools: &[ToolSchema]) -> Result<LlmResponse> {
        let (system, turns) = build_payload(&messages);
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": system,
            "messages": turns,
        });
        if !tools.is_empty() {
            body["tools"] = json!(build_tools(tools));
        }
        let data = self.call(body).await?;

        let mut text = String::new();
        let mut tool_calls = Vec::new();
        for block in data.content {
            match block.kind.as_str() {
                "text" => text.push_str(&block.text),
                "tool_use" => tool_calls.push(ToolCall {
                    id: block.id,
                    name: block.name,
                    args: block.input.unwrap_or_else(|| json!({})),
                }),
                _ => {}
            }
        }

        Ok(LlmResponse {
            text: if text.is_empty() { None } else { Some(text) },
            tool_calls,
        })
    }
}

impl AnthropicLlm {
    async fn call(&self, body: Value) -> Result<ApiResponse> {
        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("failed to call Anthropic")?
            .error_for_status()
            .context("Anthropic returned an error status")?;
        response.json().await.context("failed to parse Anthropic response")
    }
}

fn text_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter(|b| b.kind == "text")
        .map(|b| b.text.clone())
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn consecutive_tool_messages_are_merged_into_one_user_message() {
        // assistant asks for two tools, then two tool results come back.
        let messages = vec![
            Message::system("you are a bot"),
            Message::user("do stuff"),
            Message::assistant_tool_calls(
                "",
                vec![
                    ToolCall { id: "a".into(), name: "t1".into(), args: json!({"x":1}) },
                    ToolCall { id: "b".into(), name: "t2".into(), args: json!({"y":2}) },
                ],
            ),
            Message::tool_result("a", "result A"),
            Message::tool_result("b", "result B"),
        ];

        let (system, turns) = build_payload(&messages);
        assert_eq!(system, "you are a bot");
        // user, assistant(tool_use x2), user(tool_result x2)  →  3 turns
        assert_eq!(turns.len(), 3);

        // The assistant turn carries two tool_use blocks.
        let asst = &turns[1];
        assert_eq!(asst["role"], "assistant");
        assert_eq!(asst["content"].as_array().unwrap().len(), 2);
        assert_eq!(asst["content"][0]["type"], "tool_use");

        // The two tool results are merged into ONE user message.
        let merged = &turns[2];
        assert_eq!(merged["role"], "user");
        let blocks = merged["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["type"], "tool_result");
        assert_eq!(blocks[0]["tool_use_id"], "a");
        assert_eq!(blocks[1]["tool_use_id"], "b");
    }

    #[test]
    fn plain_messages_stay_plain() {
        let messages = vec![Message::user("hi"), Message::assistant("hello")];
        let (_system, turns) = build_payload(&messages);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], json!({ "role": "user", "content": "hi" }));
        assert_eq!(turns[1], json!({ "role": "assistant", "content": "hello" }));
    }
}
