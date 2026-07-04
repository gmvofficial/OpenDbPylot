//! The `Tool` trait — a capability the LLM can invoke (opendbpylot 2.0's `Tool[T]`).
//!
//! The trait is **object-safe**: `execute` receives already-parsed JSON arguments
//! (the registry validates them against `args_schema` first), so we can store
//! `Arc<dyn Tool>` without the generic-associated-type gymnastics the Python
//! version uses.

use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;

use crate::llm::ToolSchema;

/// Context passed to every tool execution.
///
/// Minimal for now — carries the conversation/request identifiers. Phase 5 will
/// add the resolved `User`; later phases add agent memory, file system, etc.
#[derive(Clone, Default, Debug)]
pub struct ToolContext {
    pub conversation_id: String,
    pub request_id: String,
}

/// What a tool returns after executing.
pub struct ToolResult {
    /// Whether the tool succeeded.
    pub success: bool,
    /// Text fed back to the LLM as the `role:"tool"` message (drives the next loop
    /// iteration). Keep it concise — large payloads belong in `ui` / the file system.
    pub result_for_llm: String,
    /// Optional structured UI payload to stream to the frontend. Phase 4 formalizes
    /// this into a typed `RichComponent`; for now it's an opaque JSON value.
    pub ui: Option<Value>,
}

impl ToolResult {
    /// A successful result with text for the LLM.
    pub fn ok(result_for_llm: impl Into<String>) -> Self {
        Self { success: true, result_for_llm: result_for_llm.into(), ui: None }
    }

    /// A failed result; the message is fed back to the LLM so it can recover.
    pub fn error(msg: impl Into<String>) -> Self {
        Self { success: false, result_for_llm: msg.into(), ui: None }
    }

    /// Attach a structured UI payload to stream alongside the text result.
    pub fn with_ui(mut self, ui: Value) -> Self {
        self.ui = Some(ui);
        self
    }
}

/// A capability the LLM can call.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique tool name (what the LLM uses to call it).
    fn name(&self) -> &str;

    /// Human/LLM-facing description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema (an `object`) describing the tool's arguments. This is exactly
    /// what the LLM sees, so field descriptions here guide the model's calls.
    fn args_schema(&self) -> Value;

    /// Groups permitted to use this tool. Empty = open to all. Enforced in Phase 5.
    fn access_groups(&self) -> Vec<String> {
        Vec::new()
    }

    /// Execute the tool with already-parsed JSON arguments.
    async fn execute(&self, ctx: &ToolContext, args: Value) -> Result<ToolResult>;

    /// Build the schema advertised to the LLM (name + description + parameters).
    fn schema(&self) -> ToolSchema {
        ToolSchema {
            name: self.name().to_string(),
            description: self.description().to_string(),
            parameters: self.args_schema(),
        }
    }
}
