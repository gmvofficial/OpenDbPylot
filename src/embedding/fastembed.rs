//! Real local semantic embeddings via [`fastembed`] (ONNX, no API key).
//!
//! Enable with `--features fastembed`. On first use the model
//! (all-MiniLM-L6-v2, ~80 MB) is downloaded to the local cache; afterwards it
//! runs fully offline. A big semantic upgrade over the hashed bag-of-words
//! [`super::local::LocalEmbedding`] — paraphrases actually land near each other.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

use super::EmbeddingService;

pub struct FastEmbedding {
    // Mutex: fastembed's session is not guaranteed Sync across versions, and
    // embedding calls are serialized per process anyway (one question at a time).
    model: Arc<Mutex<TextEmbedding>>,
}

impl FastEmbedding {
    /// Load (downloading on first use) the default all-MiniLM-L6-v2 model.
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )
        .map_err(|e| anyhow::anyhow!("loading fastembed model: {e}"))?;
        Ok(Self { model: Arc::new(Mutex::new(model)) })
    }
}

#[async_trait]
impl EmbeddingService for FastEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let model = self.model.clone();
        let text = text.to_string();
        // ONNX inference is CPU-bound and blocking — keep it off the async runtime.
        tokio::task::spawn_blocking(move || {
            let mut model = model.lock().unwrap();
            let mut out = model
                .embed(vec![text], None)
                .map_err(|e| anyhow::anyhow!("fastembed inference: {e}"))?;
            out.pop().context("fastembed returned no embedding")
        })
        .await
        .context("fastembed task panicked")?
    }
}
