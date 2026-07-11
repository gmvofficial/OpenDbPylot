//! The embedding layer: turn text into a vector of numbers so we can measure
//! how "similar" two pieces of text are. This is the heart of RAG retrieval.
//!
//! OpenDbPylot delegates embeddings to its vector store (e.g. ChromaDB). We split it
//! into its own trait so the in-memory store can use either a local, offline
//! embedding or a real API-based one.

pub mod cache;
#[cfg(feature = "fastembed")]
pub mod fastembed;
pub mod local;
pub mod openai;

use anyhow::Result;
use async_trait::async_trait;

/// The contract every embedding provider must fulfill.
#[async_trait]
pub trait EmbeddingService: Send + Sync {
    /// Turn a string into an embedding vector.
    async fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// Cosine similarity between two vectors: 1.0 = identical direction, 0.0 = unrelated.
///
/// This is the standard way to rank "how relevant" a stored item is to a query.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}
