//! The `dbpylot` command-line entry point.
//!
//! All the logic lives in `opendbpylot::cli` (shared with the Python/Node
//! bindings, which run the same CLI in-process). This binary just runs it.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    opendbpylot::cli::run().await
}
