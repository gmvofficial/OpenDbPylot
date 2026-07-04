//! OpenAI embeddings (`/v1/embeddings`). Real semantic similarity.
//!
//! Note: Anthropic has no embeddings API, so even if you generate SQL with Claude
//! you typically still use OpenAI (or a local model) for this step.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::EmbeddingService;

pub struct OpenAiEmbedding {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenAiEmbedding {
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key: api_key.into(),
            model: model.into(),
        }
    }
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

#[async_trait]
impl EmbeddingService for OpenAiEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let body = serde_json::json!({
            "model": self.model,
            "input": text,
        });

        let response = self
            .client
            .post("https://api.openai.com/v1/embeddings")
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("failed to call OpenAI embeddings")?
            .error_for_status()
            .context("OpenAI embeddings returned an error status")?;

        let data: EmbeddingResponse = response
            .json()
            .await
            .context("failed to parse OpenAI embeddings response")?;

        data.data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .context("OpenAI embeddings response was empty")
    }
}
