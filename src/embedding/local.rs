//! A deterministic, offline embedding based on hashed word counts ("bag of words").
//!
//! It is NOT semantically smart like a real model, but it's good enough for
//! retrieval based on shared words — and it needs no network or API key, so the
//! whole project runs and is testable offline. Swap in [`super::openai`] for
//! real semantic search.

use anyhow::Result;
use async_trait::async_trait;

use super::EmbeddingService;

/// Vector size. Larger = fewer hash collisions, slightly slower.
const DIM: usize = 256;

#[derive(Default)]
pub struct LocalEmbedding;

impl LocalEmbedding {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl EmbeddingService for LocalEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let mut vector = vec![0.0f32; DIM];
        for token in text
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|t| !t.is_empty())
        {
            let index = (fnv1a(token) as usize) % DIM;
            vector[index] += 1.0;
        }
        Ok(vector)
    }
}

/// FNV-1a: a tiny, fast, deterministic string hash.
fn fnv1a(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
