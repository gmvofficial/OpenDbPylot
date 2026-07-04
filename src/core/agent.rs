//! `Agent::send_message` — the tool loop. The heart of the system.
//!
//! It runs: call the LLM with the available tool schemas → if the model asks for
//! tools, execute each, feed the results back as `tool` messages, and loop → else
//! the model returned a final text answer, so emit it and stop. A hard ceiling
//! (`max_tool_iterations`) prevents runaway loops.
//!
//! `send_message` is an **async stream**: it yields `AgentEvent`s *as work happens*
//! (tool started, tool result, final text) rather than buffering until the end —
//! that's what lets the UI update live (wired up in Phase 4).

use std::sync::Arc;

use serde_json::Value;
use tokio_stream::Stream;

use crate::llm::{LlmService, Message};

use super::enhancer::ContextEnhancer;
use super::registry::ToolRegistry;
use super::tool::ToolContext;

/// Default ceiling on LLM⇄tool round-trips for one message (opendbpylot 2.0 uses 10).
const DEFAULT_MAX_TOOL_ITERATIONS: usize = 10;

/// An event emitted by the agent loop as it works. In Phase 4 these map onto
/// streamed `RichComponent`s; for now they're a clean, testable record of the run.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A tool is about to run.
    ToolStarted { name: String, args: Value },
    /// A tool finished running.
    ToolFinished {
        name: String,
        success: bool,
        result: String,
        ui: Option<Value>,
    },
    /// The model's final natural-language answer.
    FinalText(String),
    /// A non-fatal notice (e.g. an error or the iteration-limit message).
    Notice(String),
}

/// The orchestrator: an LLM + a tool registry + a system prompt (+ optional
/// per-question context enhancer for RAG grounding).
pub struct Agent {
    llm: Arc<dyn LlmService>,
    registry: Arc<ToolRegistry>,
    system_prompt: String,
    enhancer: Option<Arc<dyn ContextEnhancer>>,
    max_tool_iterations: usize,
}

impl Agent {
    pub fn new(llm: Arc<dyn LlmService>, registry: Arc<ToolRegistry>) -> Self {
        Self {
            llm,
            registry,
            system_prompt: "You are a helpful assistant. Use the available tools when they help \
                            answer the user's question."
                .to_string(),
            enhancer: None,
            max_tool_iterations: DEFAULT_MAX_TOOL_ITERATIONS,
        }
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }

    /// Attach a context enhancer (e.g. `RagEnhancer`) that appends per-question
    /// context (relevant tables/docs/examples) to the system prompt each turn.
    pub fn with_enhancer(mut self, enhancer: Arc<dyn ContextEnhancer>) -> Self {
        self.enhancer = Some(enhancer);
        self
    }

    pub fn with_max_tool_iterations(mut self, max: usize) -> Self {
        self.max_tool_iterations = max;
        self
    }

    /// Run the tool loop for one user message, streaming events as they happen.
    ///
    /// Captures cloned `Arc`s so the returned stream is `'static` and can be spawned
    /// onto a task (as the web server will do).
    pub fn send_message(
        &self,
        ctx: ToolContext,
        message: String,
    ) -> impl Stream<Item = AgentEvent> {
        self.run(ctx, Vec::new(), message)
    }

    /// Like `send_message`, but seeds the loop with prior conversation turns
    /// (`history`) so follow-up questions have context. The history is inserted
    /// between the system prompt and the new user message.
    pub fn run(
        &self,
        ctx: ToolContext,
        history: Vec<Message>,
        message: String,
    ) -> impl Stream<Item = AgentEvent> {
        let llm = self.llm.clone();
        let registry = self.registry.clone();
        let base_prompt = self.system_prompt.clone();
        let enhancer = self.enhancer.clone();
        let max_iters = self.max_tool_iterations;

        async_stream::stream! {
            let tools = registry.schemas();

            // Build the system prompt: base instructions + per-question RAG context.
            let mut system_prompt = base_prompt;
            if let Some(enh) = &enhancer {
                let extra = enh.enhance(&message).await;
                if !extra.is_empty() {
                    system_prompt.push_str("\n\n");
                    system_prompt.push_str(&extra);
                }
            }

            let mut messages = vec![Message::system(system_prompt)];
            messages.extend(history);
            messages.push(Message::user(message));

            let mut iterations = 0usize;
            let mut ran_sql = false;
            loop {
                if iterations >= max_iters {
                    yield AgentEvent::Notice(format!(
                        "Reached the tool-iteration limit ({max_iters}). Stopping."
                    ));
                    break;
                }
                iterations += 1;

                let response = match llm.chat(messages.clone(), &tools).await {
                    Ok(r) => r,
                    Err(e) => {
                        yield AgentEvent::Notice(format!("LLM error: {e}"));
                        break;
                    }
                };

                if response.is_tool_call() {
                    // Record the assistant turn (with its tool calls) for context.
                    let assistant_text = response.text.clone().unwrap_or_default();
                    messages.push(Message::assistant_tool_calls(
                        assistant_text,
                        response.tool_calls.clone(),
                    ));

                    // Execute each requested tool, streaming start/finish events and
                    // appending each result as a `tool` message for the next round.
                    for call in &response.tool_calls {
                        if call.name == "run_sql" {
                            ran_sql = true;
                        }
                        yield AgentEvent::ToolStarted {
                            name: call.name.clone(),
                            args: call.args.clone(),
                        };

                        let result = registry.execute(call, &ctx).await;

                        yield AgentEvent::ToolFinished {
                            name: call.name.clone(),
                            success: result.success,
                            result: result.result_for_llm.clone(),
                            ui: result.ui.clone(),
                        };

                        messages.push(Message::tool_result(
                            call.id.clone(),
                            result.result_for_llm,
                        ));
                    }
                    // Loop again so the model can react to the tool outputs.
                } else {
                    let text = response.text.unwrap_or_default();

                    // Recovery: sometimes the model returns SQL as *text* instead of
                    // calling run_sql. If we haven't run a query yet and the text
                    // contains a runnable read-only query, execute it rather than
                    // showing the user raw SQL.
                    if !ran_sql && registry.has("run_sql") {
                        let extracted = crate::sql::extract_sql(&text);
                        if crate::sql::is_read_only(&extracted) {
                            ran_sql = true;
                            let call = crate::llm::ToolCall {
                                id: "recovered-run-sql".to_string(),
                                name: "run_sql".to_string(),
                                args: serde_json::json!({ "sql": extracted }),
                            };
                            messages.push(Message::assistant_tool_calls(String::new(), vec![call.clone()]));
                            yield AgentEvent::ToolStarted { name: call.name.clone(), args: call.args.clone() };
                            let result = registry.execute(&call, &ctx).await;
                            yield AgentEvent::ToolFinished {
                                name: call.name.clone(),
                                success: result.success,
                                result: result.result_for_llm.clone(),
                                ui: result.ui.clone(),
                            };
                            messages.push(Message::tool_result(call.id.clone(), result.result_for_llm));
                            continue; // let the model summarize / chart the results
                        }
                    }

                    // Genuine final answer.
                    yield AgentEvent::FinalText(text);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tool::{Tool, ToolContext, ToolResult};
    use crate::llm::mock::ScriptedToolLlm;
    use crate::llm::{LlmResponse, ToolCall};
    use anyhow::Result;
    use async_trait::async_trait;
    use serde_json::json;
    use tokio_stream::StreamExt;

    /// A trivial tool that adds two numbers — proves the loop end-to-end.
    struct CalculatorTool;

    #[async_trait]
    impl Tool for CalculatorTool {
        fn name(&self) -> &str {
            "calculator"
        }
        fn description(&self) -> &str {
            "Add two numbers a and b"
        }
        fn args_schema(&self) -> Value {
            json!({
                "type": "object",
                "properties": {
                    "a": { "type": "number" },
                    "b": { "type": "number" }
                },
                "required": ["a", "b"]
            })
        }
        async fn execute(&self, _ctx: &ToolContext, args: Value) -> Result<ToolResult> {
            let a = args["a"].as_f64().unwrap_or(0.0);
            let b = args["b"].as_f64().unwrap_or(0.0);
            Ok(ToolResult::ok(format!("{}", a + b)))
        }
    }

    async fn collect_events(
        agent: &Agent,
        message: &str,
    ) -> Vec<AgentEvent> {
        let stream = agent.send_message(ToolContext::default(), message.to_string());
        tokio::pin!(stream);
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        events
    }

    #[tokio::test]
    async fn loop_executes_tool_then_returns_final_text() {
        // The model: first asks to call the calculator, then (after seeing "4")
        // returns a final answer.
        let llm = Arc::new(ScriptedToolLlm::new(vec![
            LlmResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "calculator".into(),
                    args: json!({ "a": 2, "b": 2 }),
                }],
            },
            LlmResponse::text("The answer is 4."),
        ]));

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool));
        let agent = Agent::new(llm, Arc::new(registry));

        let events = collect_events(&agent, "what is 2+2?").await;

        // Expect: ToolStarted → ToolFinished → FinalText.
        assert_eq!(events.len(), 3, "events: {events:?}");
        match &events[0] {
            AgentEvent::ToolStarted { name, args } => {
                assert_eq!(name, "calculator");
                assert_eq!(args["a"], 2);
            }
            other => panic!("expected ToolStarted, got {other:?}"),
        }
        match &events[1] {
            AgentEvent::ToolFinished { name, success, result, .. } => {
                assert_eq!(name, "calculator");
                assert!(success);
                assert_eq!(result, "4"); // the tool actually ran: 2 + 2
            }
            other => panic!("expected ToolFinished, got {other:?}"),
        }
        match &events[2] {
            AgentEvent::FinalText(t) => assert_eq!(t, "The answer is 4."),
            other => panic!("expected FinalText, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_tool_is_reported_but_loop_continues() {
        let llm = Arc::new(ScriptedToolLlm::new(vec![
            LlmResponse {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "does_not_exist".into(),
                    args: json!({}),
                }],
            },
            LlmResponse::text("Sorry, I could not do that."),
        ]));

        let registry = ToolRegistry::new(); // no tools registered
        let agent = Agent::new(llm, Arc::new(registry));

        let events = collect_events(&agent, "do something").await;

        // ToolFinished(success=false, "unknown tool") then FinalText.
        let finished = events.iter().find_map(|e| match e {
            AgentEvent::ToolFinished { success, result, .. } => Some((*success, result.clone())),
            _ => None,
        });
        let (success, result) = finished.expect("expected a ToolFinished event");
        assert!(!success);
        assert!(result.contains("unknown tool"), "got: {result}");
        assert!(matches!(events.last(), Some(AgentEvent::FinalText(_))));
    }

    #[tokio::test]
    async fn iteration_limit_stops_a_runaway_loop() {
        // The model ALWAYS asks for a tool — without a ceiling this never ends.
        let llm = Arc::new(ScriptedToolLlm::new(vec![LlmResponse {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c".into(),
                name: "calculator".into(),
                args: json!({ "a": 1, "b": 1 }),
            }],
        }]));

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool));
        let agent = Agent::new(llm, Arc::new(registry)).with_max_tool_iterations(3);

        let events = collect_events(&agent, "loop forever").await;

        // It must terminate with the limit notice (not hang).
        assert!(matches!(events.last(), Some(AgentEvent::Notice(_))));
        // 3 iterations × (ToolStarted + ToolFinished) + 1 Notice = 7 events.
        let tool_starts = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolStarted { .. }))
            .count();
        assert_eq!(tool_starts, 3);
    }
}
