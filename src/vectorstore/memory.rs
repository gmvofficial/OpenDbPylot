//! In-memory vector store. Holds everything in RAM and ranks by cosine similarity.
//!
//! A simple in-memory store with zero setup. For production you'd
//! implement [`VectorStore`] again for a persistent DB (Qdrant, pgvector, ...).

use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_trait::async_trait;

use super::VectorStore;
use crate::embedding::{cosine_similarity, EmbeddingService};
use crate::types::QuestionSql;

/// One stored item plus its precomputed embedding.
struct Entry {
    content: String,
    question: Option<String>, // only set for question/SQL pairs
    embedding: Vec<f32>,
}

pub struct MemoryVectorStore {
    embedding: Arc<dyn EmbeddingService>,
    n_results: usize,
    ddl: Mutex<Vec<Entry>>,
    docs: Mutex<Vec<Entry>>,
    sql: Mutex<Vec<Entry>>,
}

impl MemoryVectorStore {
    pub fn new(embedding: Arc<dyn EmbeddingService>) -> Self {
        Self {
            embedding,
            n_results: 10, // default n_results
            ddl: Mutex::new(Vec::new()),
            docs: Mutex::new(Vec::new()),
            sql: Mutex::new(Vec::new()),
        }
    }

    pub fn with_n_results(mut self, n: usize) -> Self {
        self.n_results = n;
        self
    }

    /// Return the indexes of the top-`n` entries most similar to `query_emb`.
    fn top_n(entries: &[Entry], query_emb: &[f32], n: usize) -> Vec<usize> {
        let mut scored: Vec<(usize, f32)> = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i, cosine_similarity(query_emb, &e.embedding)))
            .collect();
        // Highest similarity first.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(n).map(|(i, _)| i).collect()
    }
}

#[async_trait]
impl VectorStore for MemoryVectorStore {
    async fn add_ddl(&self, ddl: &str) -> Result<()> {
        let embedding = self.embedding.embed(ddl).await?;
        self.ddl.lock().unwrap().push(Entry {
            content: ddl.to_string(),
            question: None,
            embedding,
        });
        Ok(())
    }

    async fn add_documentation(&self, doc: &str) -> Result<()> {
        let embedding = self.embedding.embed(doc).await?;
        self.docs.lock().unwrap().push(Entry {
            content: doc.to_string(),
            question: None,
            embedding,
        });
        Ok(())
    }

    async fn clear_ddl(&self) -> Result<()> {
        self.ddl.lock().unwrap().clear();
        Ok(())
    }

    async fn add_question_sql(&self, question: &str, sql: &str) -> Result<()> {
        // We embed the QUESTION (that's what new questions are matched against),
        // and keep the SQL as the stored content.
        let embedding = self.embedding.embed(question).await?;
        self.sql.lock().unwrap().push(Entry {
            content: sql.to_string(),
            question: Some(question.to_string()),
            embedding,
        });
        Ok(())
    }

    async fn get_related_ddl(&self, question: &str) -> Result<Vec<String>> {
        let query = self.embedding.embed(question).await?;
        let guard = self.ddl.lock().unwrap();
        let idxs = Self::top_n(guard.as_slice(), &query, self.n_results);
        Ok(idxs.into_iter().map(|i| guard[i].content.clone()).collect())
    }

    async fn get_related_documentation(&self, question: &str) -> Result<Vec<String>> {
        let query = self.embedding.embed(question).await?;
        let guard = self.docs.lock().unwrap();
        let idxs = Self::top_n(guard.as_slice(), &query, self.n_results);
        Ok(idxs.into_iter().map(|i| guard[i].content.clone()).collect())
    }

    async fn get_similar_question_sql(&self, question: &str) -> Result<Vec<QuestionSql>> {
        let query = self.embedding.embed(question).await?;
        let guard = self.sql.lock().unwrap();
        let idxs = Self::top_n(guard.as_slice(), &query, self.n_results);
        Ok(idxs
            .into_iter()
            .map(|i| QuestionSql {
                question: guard[i].question.clone().unwrap_or_default(),
                sql: guard[i].content.clone(),
            })
            .collect())
    }

    async fn all_ddl(&self) -> Result<Vec<String>> {
        Ok(self.ddl.lock().unwrap().iter().map(|e| e.content.clone()).collect())
    }

    async fn all_documentation(&self) -> Result<Vec<String>> {
        Ok(self.docs.lock().unwrap().iter().map(|e| e.content.clone()).collect())
    }

    async fn all_question_sql(&self) -> Result<Vec<QuestionSql>> {
        Ok(self
            .sql
            .lock()
            .unwrap()
            .iter()
            .map(|e| QuestionSql {
                question: e.question.clone().unwrap_or_default(),
                sql: e.content.clone(),
            })
            .collect())
    }
}
