//! Conversation memory + chat history.
//!
//! Two roles:
//! - **Follow-up context** for the RAG core (`recent`/`append`), as before.
//! - **Chat history** for the UI: list conversations, load a conversation's turns,
//!   and give each a title — like a normal chat tool's sidebar.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::QuestionSql;

/// Lightweight conversation summary for the sidebar list.
#[derive(Clone, Serialize, Deserialize)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub updated_ms: u128,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Conversation {
    title: String,
    updated_ms: u128,
    turns: Vec<QuestionSql>,
}

fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

#[async_trait]
pub trait ConversationStore: Send + Sync {
    /// Up to `limit` most recent turns (oldest first) — used for follow-up context.
    async fn recent(&self, conversation_id: &str, limit: usize) -> Result<Vec<QuestionSql>>;
    /// Append a completed turn.
    async fn append(&self, conversation_id: &str, turn: QuestionSql) -> Result<()>;

    // --- chat-history API (default no-ops so simple stores still compile) ---
    async fn list(&self) -> Result<Vec<ConversationMeta>> {
        Ok(Vec::new())
    }
    async fn turns(&self, conversation_id: &str) -> Result<Vec<QuestionSql>> {
        self.recent(conversation_id, usize::MAX).await
    }
    /// Create the conversation if new, setting its title (first question).
    async fn ensure(&self, _conversation_id: &str, _title: &str) -> Result<()> {
        Ok(())
    }
    /// Delete a conversation and all its history. Default no-op.
    async fn delete(&self, _conversation_id: &str) -> Result<()> {
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// In-memory
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct MemoryConversationStore {
    conversations: Mutex<HashMap<String, Conversation>>,
}

impl MemoryConversationStore {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl ConversationStore for MemoryConversationStore {
    async fn recent(&self, id: &str, limit: usize) -> Result<Vec<QuestionSql>> {
        let g = self.conversations.lock().unwrap();
        Ok(g.get(id)
            .map(|c| {
                let start = c.turns.len().saturating_sub(limit);
                c.turns[start..].to_vec()
            })
            .unwrap_or_default())
    }

    async fn append(&self, id: &str, turn: QuestionSql) -> Result<()> {
        let mut g = self.conversations.lock().unwrap();
        let c = g.entry(id.to_string()).or_default();
        if c.title.is_empty() {
            c.title = turn.question.clone();
        }
        c.turns.push(turn);
        c.updated_ms = now_ms();
        Ok(())
    }

    async fn list(&self) -> Result<Vec<ConversationMeta>> {
        let g = self.conversations.lock().unwrap();
        let mut v: Vec<ConversationMeta> = g
            .iter()
            .map(|(id, c)| ConversationMeta { id: id.clone(), title: c.title.clone(), updated_ms: c.updated_ms })
            .collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        Ok(v)
    }

    async fn turns(&self, id: &str) -> Result<Vec<QuestionSql>> {
        Ok(self.conversations.lock().unwrap().get(id).map(|c| c.turns.clone()).unwrap_or_default())
    }

    async fn ensure(&self, id: &str, title: &str) -> Result<()> {
        let mut g = self.conversations.lock().unwrap();
        let c = g.entry(id.to_string()).or_default();
        if c.title.is_empty() {
            c.title = title.to_string();
            c.updated_ms = now_ms();
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        self.conversations.lock().unwrap().remove(id);
        Ok(())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// File-backed (persistent)
// ─────────────────────────────────────────────────────────────────────────────

pub struct FileConversationStore {
    path: PathBuf,
    data: Mutex<HashMap<String, Conversation>>,
}

impl FileConversationStore {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let data: HashMap<String, Conversation> = if path.exists() {
            serde_json::from_slice(&std::fs::read(&path)?).unwrap_or_default()
        } else {
            HashMap::new()
        };
        Ok(Self { path, data: Mutex::new(data) })
    }

    fn save(&self, data: &HashMap<String, Conversation>) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_vec_pretty(data)?)?;
        Ok(())
    }
}

#[async_trait]
impl ConversationStore for FileConversationStore {
    async fn recent(&self, id: &str, limit: usize) -> Result<Vec<QuestionSql>> {
        let g = self.data.lock().unwrap();
        Ok(g.get(id)
            .map(|c| {
                let start = c.turns.len().saturating_sub(limit);
                c.turns[start..].to_vec()
            })
            .unwrap_or_default())
    }

    async fn append(&self, id: &str, turn: QuestionSql) -> Result<()> {
        let mut g = self.data.lock().unwrap();
        let c = g.entry(id.to_string()).or_default();
        if c.title.is_empty() {
            c.title = turn.question.clone();
        }
        c.turns.push(turn);
        c.updated_ms = now_ms();
        self.save(&g)
    }

    async fn list(&self) -> Result<Vec<ConversationMeta>> {
        let g = self.data.lock().unwrap();
        let mut v: Vec<ConversationMeta> = g
            .iter()
            .map(|(id, c)| ConversationMeta { id: id.clone(), title: c.title.clone(), updated_ms: c.updated_ms })
            .collect();
        v.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms));
        Ok(v)
    }

    async fn turns(&self, id: &str) -> Result<Vec<QuestionSql>> {
        Ok(self.data.lock().unwrap().get(id).map(|c| c.turns.clone()).unwrap_or_default())
    }

    async fn ensure(&self, id: &str, title: &str) -> Result<()> {
        let mut g = self.data.lock().unwrap();
        let c = g.entry(id.to_string()).or_default();
        if c.title.is_empty() {
            c.title = title.to_string();
            c.updated_ms = now_ms();
            self.save(&g)?;
        }
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut g = self.data.lock().unwrap();
        if g.remove(id).is_some() {
            self.save(&g)?;
        }
        Ok(())
    }
}
