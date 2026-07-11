//! Ollama implementation of `LlmService` — talk to a local model (no API key).
//!
//! Requires a running Ollama instance (default http://localhost:11434) with a
//! pulled model, e.g. `ollama pull llama3`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

use super::{LlmService, Message};

/// Local models on modest hardware can be slow — allow much longer than the
/// hosted APIs before declaring the request hung.
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

pub struct OllamaLlm {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaLlm {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            client: super::http_client(DEFAULT_TIMEOUT),
            base_url: "http://localhost:11434".to_string(),
            model: model.into(),
        }
    }

    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Override the request timeout (from user settings).
    pub fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.client = super::http_client(timeout);
        self
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ResponseMessage,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

#[async_trait]
impl LlmService for OllamaLlm {
    async fn submit_prompt(&self, messages: Vec<Message>) -> Result<String> {
        // Ollama accepts our {role, content} messages directly (incl. system).
        let body = serde_json::json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
        });

        let url = format!("{}/api/chat", self.base_url);
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .context("failed to call Ollama (is it running?)")?
            .error_for_status()
            .context("Ollama returned an error status")?;

        let data: ChatResponse = response
            .json()
            .await
            .context("failed to parse Ollama response")?;

        Ok(data.message.content)
    }
}
