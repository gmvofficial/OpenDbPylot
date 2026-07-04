//! `ToolRegistry` — the single choke point for tool validation and execution.
//!
//! Holds the registered tools, exposes the schemas the LLM may see, and executes a
//! `ToolCall` by name. This is where permission checks, `transform_args`
//! (row-level security), and audit logging will hang off in Phase 5 — keeping all
//! cross-cutting concerns in one place.

use std::sync::Arc;

use crate::llm::{ToolCall, ToolSchema};

use super::tool::{Tool, ToolContext, ToolResult};

/// Holds tools in registration order (for deterministic schema output) and runs them.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Register a tool.
    ///
    /// Named `register` deliberately and kept that way — opendbpylot-main drifted into a
    /// confusing `register` vs `register_local_tool` split (PROJECT_STUDY §5.11).
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        // Replace any existing tool with the same name (last registration wins).
        if let Some(slot) = self.tools.iter_mut().find(|t| t.name() == tool.name()) {
            *slot = tool;
        } else {
            self.tools.push(tool);
        }
    }

    /// The tool schemas advertised to the LLM, in registration order.
    /// Phase 5 will filter these by the resolved user's permissions.
    pub fn schemas(&self) -> Vec<ToolSchema> {
        self.tools.iter().map(|t| t.schema()).collect()
    }

    /// Whether any tools are registered.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Whether a tool with this name is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name() == name)
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    /// Validate and execute a tool call. Never panics: an unknown tool or an
    /// execution error is converted into a failed `ToolResult` so the agent loop
    /// can feed it back to the model and let it recover.
    pub async fn execute(&self, call: &ToolCall, ctx: &ToolContext) -> ToolResult {
        let tool = match self.get(&call.name) {
            Some(t) => t,
            None => return ToolResult::error(format!("unknown tool: {}", call.name)),
        };

        // (Phase 5 inserts: permission check + transform_args + audit here.)

        match tool.execute(ctx, call.args.clone()).await {
            Ok(result) => result,
            Err(e) => ToolResult::error(format!("tool '{}' failed: {e}", call.name)),
        }
    }
}
