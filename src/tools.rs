//! Concrete, built-in tools the agent can call (opendbpylot 2.0's `tools/`).
//!
//! `run_sql`        — execute SQL against the connected database.
//! `visualize_data` — chart the results of a previous `run_sql` call.

pub mod memory;
pub mod run_sql;
pub mod visualize_data;
