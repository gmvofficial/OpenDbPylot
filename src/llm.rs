//! The LLM layer: anything that can turn a prompt into text — or into tool calls.
//!
//! implementation of `submit_prompt` (legacy) plus the 2.0 tool-calling
//! interface. The `chat()` method is the tool-aware entry point used by the agent
//! loop; `submit_prompt()` remains for the legacy single-shot RAG path.

pub mod anthropic;
pub mod mock;
pub mod ollama;
pub mod openai;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One turn in a conversation sent to the LLM.
///
/// `role` is "system", "user", "assistant", or "tool". The optional `tool_calls`
/// (on an assistant turn) and `tool_call_id` (on a tool-result turn) carry the
/// tool-calling shape that vendor APIs need. Plain turns serialize to the classic
/// `{role, content}` object — the extra fields are skipped when empty, so the
/// legacy `submit_prompt` path is unaffected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    /// Set on an `assistant` turn when the model requested tool calls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// Set on a `tool` turn — the id of the call this message answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self::plain("system", content)
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self::plain("user", content)
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::plain("assistant", content)
    }

    fn plain(role: &str, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), tool_calls: Vec::new(), tool_call_id: None }
    }

    /// An assistant turn that requested one or more tool calls.
    pub fn assistant_tool_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self { role: "assistant".into(), content: content.into(), tool_calls, tool_call_id: None }
    }

    /// A tool-result turn answering a specific tool call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}

/// A tool definition advertised to the model so it knows what it can call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    /// JSON Schema (an `object`) describing the tool's arguments.
    pub parameters: Value,
}

/// A request from the model to call a tool, with parsed arguments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned id, echoed back in the matching tool-result message.
    pub id: String,
    pub name: String,
    /// The tool arguments as a JSON object.
    pub args: Value,
}

/// The model's reply: free text and/or one or more tool calls.
#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl LlmResponse {
    /// Convenience constructor for a plain text reply (no tools).
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: Some(text.into()), tool_calls: Vec::new() }
    }

    /// `true` if the model asked to call at least one tool.
    pub fn is_tool_call(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

/// The contract every LLM provider must fulfill.
///
/// `Send + Sync` so it can be shared across async tasks (e.g. inside a web server).
#[async_trait]
pub trait LlmService: Send + Sync {
    /// Legacy single-shot path: send messages, get back the model's text reply.
    /// Used by `OpenDbPylot::generate_sql` (the RAG path) until the agent loop replaces it.
    async fn submit_prompt(&self, messages: Vec<Message>) -> Result<String>;

    /// Tool-aware path used by the agent loop. Sends the conversation plus the
    /// available tool schemas and returns text and/or tool calls.
    ///
    /// The default implementation ignores `tools` and wraps `submit_prompt`, so
    /// text-only adapters (e.g. Ollama) still work — they just never emit tool
    /// calls. Tool-capable adapters (Anthropic, OpenAI) override this.
    async fn chat(&self, messages: Vec<Message>, _tools: &[ToolSchema]) -> Result<LlmResponse> {
        let text = self.submit_prompt(messages).await?;
        Ok(LlmResponse::text(text))
    }
}
