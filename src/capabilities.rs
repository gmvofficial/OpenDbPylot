//! Capabilities — pluggable services that *tools* depend on (opendbpylot 2.0's
//! `capabilities/`). A capability is the swappable backend behind a tool: the
//! `SqlRunner` (already at `crate::sqlrunner`) is one; `FileSystem` is another,
//! used for the run_sql → visualize_data hand-off.

pub mod file_system;
