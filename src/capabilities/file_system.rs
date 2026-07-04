//! `FileSystem` capability — a tiny key→content store used to hand large result
//! sets between tools without routing them through the LLM token stream.
//!
//! `run_sql` writes the full result set to a file and tells the model only the
//! filename; `visualize_data` reads that file back. This is opendbpylot 2.0's CSV
//! hand-off (PROJECT_STUDY §5.3). We default to an in-memory implementation —
//! the file only needs to live for the duration of a request, and it keeps the
//! demo free of temp-file clutter.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{anyhow, Result};
use async_trait::async_trait;

#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Write (or overwrite) a file.
    async fn write_file(&self, name: &str, content: &str) -> Result<()>;
    /// Read a file's contents, erroring if it doesn't exist.
    async fn read_file(&self, name: &str) -> Result<String>;
}

/// Thread-safe in-memory file store (the default).
#[derive(Default)]
pub struct MemoryFileSystem {
    files: Mutex<HashMap<String, String>>,
}

impl MemoryFileSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl FileSystem for MemoryFileSystem {
    async fn write_file(&self, name: &str, content: &str) -> Result<()> {
        self.files.lock().unwrap().insert(name.to_string(), content.to_string());
        Ok(())
    }

    async fn read_file(&self, name: &str) -> Result<String> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow!("file not found: {name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_then_read_roundtrips() {
        let fs = MemoryFileSystem::new();
        fs.write_file("a.json", "{\"x\":1}").await.unwrap();
        assert_eq!(fs.read_file("a.json").await.unwrap(), "{\"x\":1}");
        assert!(fs.read_file("missing").await.is_err());
    }
}
