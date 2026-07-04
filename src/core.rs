//! The agent framework — opendbpylot 2.0's hexagonal core, ported to Rust.
//!
//! `tool`     — the `Tool` trait: a capability the LLM can invoke.
//! `registry` — `ToolRegistry`: the single choke point that validates and executes
//!              tool calls (and, later, enforces permissions + audit).
//! `agent`    — `Agent::send_message`: the tool loop (LLM ⇄ tools) that streams
//!              events as work happens. This is the heart of the system.

pub mod agent;
pub mod enhancer;
pub mod registry;
pub mod system_prompt;
pub mod tool;
