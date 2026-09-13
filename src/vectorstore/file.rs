//! A JSON-file-backed vector store, so training data **survives restarts**.
//!
//! Same retrieval logic as the in-memory store, but it loads from a file on
//! startup and saves after every change. For heavier production use you'd swap in
//! a real vector DB (Qdrant / pgvector) — just implement [`VectorStore`] again.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::VectorStore;
use crate::embedding::EmbeddingService;
use crate::retrieval::hybrid_top_n;
use crate::types::QuestionSql;

#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    content: String,
    question: Option<String>,
    embedding: Vec<f32>,
}

#[derive(Default, Serialize, Deserialize)]
struct Data {
    ddl: Vec<Entry>,
    docs: Vec<Entry>,
    sql: Vec<Entry>,
}

pub struct FileVectorStore {
    path: PathBuf,
    embedding: Arc<dyn EmbeddingService>,
    n_results: usize,
    data: Mutex<Data>,
}

impl FileVectorStore {
    /// Open (or create) a store at `path`, loading any existing data.
    pub fn new(path: impl Into<PathBuf>, embedding: Arc<dyn EmbeddingService>) -> Result<Self> {
        let path = path.into();
        let data = if path.exists() {
            let bytes = std::fs::read(&path).context("reading vector store file")?;
            serde_json::from_slice(&bytes).context("parsing vector store file")?
        } else {
            Data::default()
        };
        Ok(Self {
            path,
            embedding,
            // Wider than the prompt needs, because reranking cuts it back —
            // a reranker can only reorder what retrieval handed it.
            n_results: 24,
            data: Mutex::new(data),
        })
    }

    pub fn with_n_results(mut self, n: usize) -> Self {
        self.n_results = n;
        self
    }

    fn save(&self, data: &Data) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(data).context("serializing vector store")?;
        std::fs::write(&self.path, bytes).context("writing vector store file")?;
        Ok(())
    }

    /// Hybrid-rank entries against the query; returns top-`n` indices.
    /// For Q/SQL pairs the *question* is the searchable text, not the SQL.
    fn top_n(entries: &[Entry], query_text: &str, query_emb: &[f32], n: usize) -> Vec<usize> {
        let texts: Vec<&str> = entries
            .iter()
            .map(|e| e.question.as_deref().unwrap_or(&e.content))
            .collect();
        let embs: Vec<&[f32]> = entries.iter().map(|e| e.embedding.as_slice()).collect();
        hybrid_top_n(query_text, query_emb, &texts, &embs, n)
    }
}

#[async_trait]
impl VectorStore for FileVectorStore {
    async fn add_ddl(&self, ddl: &str) -> Result<()> {
        let embedding = self.embedding.embed(ddl).await?;
        let mut data = self.data.lock().unwrap();
        data.ddl.push(Entry { content: ddl.to_string(), question: None, embedding });
        self.save(&data)
    }

    async fn add_documentation(&self, doc: &str) -> Result<()> {
        let embedding = self.embedding.embed(doc).await?;
        let mut data = self.data.lock().unwrap();
        data.docs.push(Entry { content: doc.to_string(), question: None, embedding });
        self.save(&data)
    }

    async fn clear_ddl(&self) -> Result<()> {
        let mut data = self.data.lock().unwrap();
        data.ddl.clear();
        self.save(&data)
    }

    async fn add_question_sql(&self, question: &str, sql: &str) -> Result<()> {
        let embedding = self.embedding.embed(question).await?;
        let mut data = self.data.lock().unwrap();
        data.sql.push(Entry {
            content: sql.to_string(),
            question: Some(question.to_string()),
            embedding,
        });
        self.save(&data)
    }

    async fn get_related_ddl(&self, question: &str) -> Result<Vec<String>> {
        let query = self.embedding.embed(question).await?;
        let data = self.data.lock().unwrap();
        let idxs = Self::top_n(&data.ddl, question, &query, self.n_results);
        Ok(idxs.into_iter().map(|i| data.ddl[i].content.clone()).collect())
    }

    async fn get_related_documentation(&self, question: &str) -> Result<Vec<String>> {
        let query = self.embedding.embed(question).await?;
        let data = self.data.lock().unwrap();
        let idxs = Self::top_n(&data.docs, question, &query, self.n_results);
        Ok(idxs.into_iter().map(|i| data.docs[i].content.clone()).collect())
    }

    async fn get_similar_question_sql(&self, question: &str) -> Result<Vec<QuestionSql>> {
        let query = self.embedding.embed(question).await?;
        let data = self.data.lock().unwrap();
        let idxs = Self::top_n(&data.sql, question, &query, self.n_results);
        Ok(idxs
            .into_iter()
            .map(|i| QuestionSql {
                question: data.sql[i].question.clone().unwrap_or_default(),
                sql: data.sql[i].content.clone(),
            })
            .collect())
    }

    async fn all_ddl(&self) -> Result<Vec<String>> {
        Ok(self.data.lock().unwrap().ddl.iter().map(|e| e.content.clone()).collect())
    }

    async fn all_documentation(&self) -> Result<Vec<String>> {
        Ok(self.data.lock().unwrap().docs.iter().map(|e| e.content.clone()).collect())
    }

    async fn all_question_sql(&self) -> Result<Vec<QuestionSql>> {
        Ok(self
            .data
            .lock()
            .unwrap()
            .sql
            .iter()
            .map(|e| QuestionSql {
                question: e.question.clone().unwrap_or_default(),
                sql: e.content.clone(),
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::local::LocalEmbedding;

    #[tokio::test]
    async fn persists_across_instances() {
        let path = std::env::temp_dir().join(format!("opendbpylot_store_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        {
            let store = FileVectorStore::new(path.clone(), Arc::new(LocalEmbedding::new())).unwrap();
            store
                .add_ddl("CREATE TABLE users (id INT, country TEXT);")
                .await
                .unwrap();
        } // store dropped — simulates process exit

        let reopened = FileVectorStore::new(path.clone(), Arc::new(LocalEmbedding::new())).unwrap();
        let ddl = reopened.get_related_ddl("users country").await.unwrap();
        assert!(!ddl.is_empty(), "training data should survive a restart");

        let _ = std::fs::remove_file(&path);
    }
}
