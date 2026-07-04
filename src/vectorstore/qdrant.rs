//! Qdrant vector-database backend — a real, scalable, persistent KB.
//!
//! Same `VectorStore` contract as the in-memory store, but vectors live in a
//! Qdrant server (approximate-nearest-neighbour search that scales to millions of
//! items). One collection holds all three kinds, tagged by a `kind` payload field.
//!
//! Enable with `--features qdrant`. Run Qdrant locally with:
//!   docker run -p 6333:6333 -p 6334:6334 qdrant/qdrant
//! and point at the gRPC port, e.g. `http://localhost:6334`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use async_trait::async_trait;
use qdrant_client::qdrant::{
    Condition, CreateCollectionBuilder, Distance, Filter, PointStruct, SearchPointsBuilder,
    UpsertPointsBuilder, VectorParamsBuilder,
};
use qdrant_client::{Payload, Qdrant};
use serde_json::json;

use super::VectorStore;
use crate::embedding::EmbeddingService;
use crate::types::QuestionSql;

pub struct QdrantVectorStore {
    client: Qdrant,
    collection: String,
    embedding: Arc<dyn EmbeddingService>,
    n_results: u64,
    counter: AtomicU64,
}

impl QdrantVectorStore {
    /// Connect to Qdrant and ensure the collection exists (created lazily, with a
    /// vector size matching the embedding model in use).
    pub async fn new(
        url: &str,
        collection: impl Into<String>,
        embedding: Arc<dyn EmbeddingService>,
    ) -> Result<Self> {
        let client = Qdrant::from_url(url).build().context("connecting to Qdrant")?;
        let collection = collection.into();

        // Probe the embedding dimension so the collection matches the model.
        let dim = embedding.embed("schema").await?.len() as u64;

        if !client.collection_exists(&collection).await? {
            client
                .create_collection(
                    CreateCollectionBuilder::new(&collection)
                        .vectors_config(VectorParamsBuilder::new(dim, Distance::Cosine)),
                )
                .await
                .context("creating Qdrant collection")?;
        }

        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);

        Ok(Self {
            client,
            collection,
            embedding,
            n_results: 10,
            counter: AtomicU64::new(seed),
        })
    }

    pub fn with_n_results(mut self, n: u64) -> Self {
        self.n_results = n;
        self
    }

    async fn add(&self, kind: &str, content: &str, question: &str, embed_text: &str) -> Result<()> {
        let vector = self.embedding.embed(embed_text).await?;
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        let payload: Payload = json!({
            "kind": kind,
            "content": content,
            "question": question,
        })
        .try_into()
        .context("building Qdrant payload")?;

        let point = PointStruct::new(id, vector, payload);
        self.client
            .upsert_points(UpsertPointsBuilder::new(&self.collection, vec![point]))
            .await
            .context("upserting into Qdrant")?;
        Ok(())
    }

    /// Returns (content, question) pairs for the top matches of a given kind.
    async fn search(&self, kind: &str, query: &str) -> Result<Vec<(String, String)>> {
        let vector = self.embedding.embed(query).await?;
        let filter = Filter::must([Condition::matches("kind", kind.to_string())]);

        let response = self
            .client
            .search_points(
                SearchPointsBuilder::new(&self.collection, vector, self.n_results)
                    .filter(filter)
                    .with_payload(true),
            )
            .await
            .context("searching Qdrant")?;

        let field = |p: &std::collections::HashMap<String, qdrant_client::qdrant::Value>, k: &str| {
            p.get(k).and_then(|v| v.as_str()).map(|s| s.to_string()).unwrap_or_default()
        };

        Ok(response
            .result
            .into_iter()
            .map(|sp| (field(&sp.payload, "content"), field(&sp.payload, "question")))
            .collect())
    }
}

#[async_trait]
impl VectorStore for QdrantVectorStore {
    async fn add_ddl(&self, ddl: &str) -> Result<()> {
        self.add("ddl", ddl, "", ddl).await
    }

    async fn add_documentation(&self, doc: &str) -> Result<()> {
        self.add("doc", doc, "", doc).await
    }

    async fn add_question_sql(&self, question: &str, sql: &str) -> Result<()> {
        // Embed the QUESTION (matched against new questions); store the SQL as content.
        self.add("sql", sql, question, question).await
    }

    async fn get_related_ddl(&self, question: &str) -> Result<Vec<String>> {
        Ok(self.search("ddl", question).await?.into_iter().map(|(c, _)| c).collect())
    }

    async fn get_related_documentation(&self, question: &str) -> Result<Vec<String>> {
        Ok(self.search("doc", question).await?.into_iter().map(|(c, _)| c).collect())
    }

    async fn get_similar_question_sql(&self, question: &str) -> Result<Vec<QuestionSql>> {
        Ok(self
            .search("sql", question)
            .await?
            .into_iter()
            .map(|(content, question)| QuestionSql { question, sql: content })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::local::LocalEmbedding;

    // Requires a running Qdrant (gRPC at $QDRANT_URL or localhost:6334).
    #[tokio::test]
    async fn qdrant_roundtrip() {
        let url = std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".into());
        let store = QdrantVectorStore::new(
            &url,
            format!("opendbpylot_test_{}", std::process::id()),
            Arc::new(LocalEmbedding::new()),
        )
        .await
        .unwrap();

        store.add_ddl("CREATE TABLE users (id INT, country TEXT);").await.unwrap();
        store.add_documentation("country is the user's country").await.unwrap();
        store
            .add_question_sql("users per country", "SELECT country, COUNT(*) FROM users GROUP BY country;")
            .await
            .unwrap();

        // Qdrant indexes asynchronously; give it a moment.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;

        let ddl = store.get_related_ddl("country users table").await.unwrap();
        assert!(!ddl.is_empty(), "should retrieve DDL");

        let qs = store.get_similar_question_sql("how many users per country").await.unwrap();
        assert!(!qs.is_empty(), "should retrieve similar question/SQL");
    }
}
