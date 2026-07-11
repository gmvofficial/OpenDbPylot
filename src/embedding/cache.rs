//! File-backed cache decorator for any [`EmbeddingService`].
//!
//! Embedding the same text twice is pure waste — and with a paid provider it
//! costs real money. The cache is keyed by `(tag, text)` where `tag` names the
//! provider+model (vectors from different models must never mix), held in
//! memory, and appended to a JSONL file so it survives restarts.
//!
//! Note the biggest win: the ask path embeds the *same question* three times
//! (related DDL, related docs, similar Q/SQL) — with the cache only the first
//! call hits the provider.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::EmbeddingService;

#[derive(Serialize, Deserialize)]
struct Line {
    /// Cache key as hex (FNV-1a over `tag \0 text`).
    k: String,
    e: Vec<f32>,
}

pub struct CachedEmbedding {
    inner: std::sync::Arc<dyn EmbeddingService>,
    tag: String,
    /// `None` = in-memory only (used in tests).
    path: Option<PathBuf>,
    mem: Mutex<HashMap<u64, Vec<f32>>>,
}

impl CachedEmbedding {
    /// Wrap `inner`, persisting to `path` (created on first write; parent dirs
    /// are created). Existing cache lines are loaded eagerly; corrupt lines are
    /// skipped — a damaged cache degrades to a slower start, never an error.
    pub fn new(
        inner: std::sync::Arc<dyn EmbeddingService>,
        tag: impl Into<String>,
        path: Option<PathBuf>,
    ) -> Self {
        let mut mem = HashMap::new();
        if let Some(p) = &path {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(content) = std::fs::read_to_string(p) {
                for line in content.lines() {
                    if let Ok(l) = serde_json::from_str::<Line>(line) {
                        if let Ok(k) = u64::from_str_radix(&l.k, 16) {
                            mem.insert(k, l.e);
                        }
                    }
                }
            }
        }
        Self { inner, tag: tag.into(), path, mem: Mutex::new(mem) }
    }

    fn key(&self, text: &str) -> u64 {
        // FNV-1a over tag + NUL + text.
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in self.tag.bytes().chain([0u8]).chain(text.bytes()) {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        hash
    }

    fn persist(&self, key: u64, embedding: &[f32]) {
        let Some(path) = &self.path else { return };
        let line = Line { k: format!("{key:016x}"), e: embedding.to_vec() };
        // Append-only; a failed write only loses the cache entry, never the result.
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            if let Ok(json) = serde_json::to_string(&line) {
                let _ = writeln!(f, "{json}");
            }
        }
    }
}

#[async_trait]
impl EmbeddingService for CachedEmbedding {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let key = self.key(text);
        if let Some(hit) = self.mem.lock().unwrap().get(&key) {
            return Ok(hit.clone());
        }
        let embedding = self.inner.embed(text).await?;
        self.mem.lock().unwrap().insert(key, embedding.clone());
        self.persist(key, &embedding);
        Ok(embedding)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;

    /// Counts calls so tests can prove the cache short-circuits.
    struct CountingEmbedding {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl EmbeddingService for CountingEmbedding {
        async fn embed(&self, text: &str) -> Result<Vec<f32>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(vec![text.len() as f32, 1.0])
        }
    }

    #[tokio::test]
    async fn repeated_texts_hit_the_cache() {
        let inner = Arc::new(CountingEmbedding { calls: AtomicUsize::new(0) });
        let cache = CachedEmbedding::new(inner.clone(), "test:model", None);

        let a1 = cache.embed("how many orders?").await.unwrap();
        let a2 = cache.embed("how many orders?").await.unwrap();
        let _b = cache.embed("different text").await.unwrap();

        assert_eq!(a1, a2);
        assert_eq!(inner.calls.load(Ordering::SeqCst), 2); // 2 unique texts, 3 calls
    }

    #[tokio::test]
    async fn cache_survives_restart_via_jsonl() {
        let path = std::env::temp_dir().join(format!(
            "opendbpylot_embcache_test_{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let inner = Arc::new(CountingEmbedding { calls: AtomicUsize::new(0) });
            let cache = CachedEmbedding::new(inner, "test:model", Some(path.clone()));
            cache.embed("persist me").await.unwrap();
        } // dropped — simulates process exit

        let inner = Arc::new(CountingEmbedding { calls: AtomicUsize::new(0) });
        let cache = CachedEmbedding::new(inner.clone(), "test:model", Some(path.clone()));
        let v = cache.embed("persist me").await.unwrap();

        assert_eq!(v, vec![10.0, 1.0]); // the persisted vector, not a recomputed one
        assert_eq!(inner.calls.load(Ordering::SeqCst), 0, "must be served from disk");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn different_tags_do_not_collide() {
        let inner = Arc::new(CountingEmbedding { calls: AtomicUsize::new(0) });
        let a = CachedEmbedding::new(inner.clone(), "model-a", None);
        let b = CachedEmbedding::new(inner.clone(), "model-b", None);

        a.embed("same text").await.unwrap();
        b.embed("same text").await.unwrap();
        // Separate instances → separate memories anyway, but the *file* key must
        // differ too — prove the key incorporates the tag.
        assert_ne!(a.key("same text"), b.key("same text"));
    }
}
