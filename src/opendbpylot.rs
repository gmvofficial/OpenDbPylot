//! The orchestrator — the central engine that ties the pipeline together.
//!
//! It wires together the four swappable pieces (LLM, vector store, SQL runner)
//! and exposes the two methods that matter: `train` and `ask`.

use std::sync::Arc;

use anyhow::Result;

use crate::conversation::ConversationStore;
use crate::llm::LlmService;
use crate::prompt::build_sql_prompt;
use crate::sql::{extract_sql, is_sql_valid};
use crate::sqlrunner::{QueryResult, SqlRunner};
use crate::types::QuestionSql;
use crate::vectorstore::VectorStore;

/// Tunable engine settings.
pub struct OpenDbPylotConfig {
    /// SQL dialect named in the prompt (e.g. "SQLite", "PostgreSQL").
    pub dialect: String,
    /// After a successful question, store it as a new training example.
    pub auto_train: bool,
    /// Allow the LLM to run an exploratory ("intermediate") query and see its
    /// results before writing the final SQL. Off by default for privacy.
    pub allow_llm_to_see_data: bool,
    /// How many prior conversation turns to include as context for follow-ups.
    pub history_limit: usize,
}

impl Default for OpenDbPylotConfig {
    fn default() -> Self {
        Self {
            dialect: "SQLite".into(),
            // Off by default: blanket-saving every answered question as "training"
            // pollutes the knowledge base with whatever SQL the model happened to
            // produce. Saving examples should be a deliberate action.
            auto_train: false,
            allow_llm_to_see_data: false,
            history_limit: 5,
        }
    }
}

/// What `ask` returns: the generated SQL and (if a runner is set) the rows.
#[derive(Debug)]
pub struct AskResult {
    pub sql: String,
    pub result: Option<QueryResult>,
}

pub struct OpenDbPylot {
    llm: Arc<dyn LlmService>,
    store: Arc<dyn VectorStore>,
    runner: Option<Arc<dyn SqlRunner>>,
    conversations: Option<Arc<dyn ConversationStore>>,
    config: OpenDbPylotConfig,
}

impl OpenDbPylot {
    pub fn new(llm: Arc<dyn LlmService>, store: Arc<dyn VectorStore>) -> Self {
        Self {
            llm,
            store,
            runner: None,
            conversations: None,
            config: OpenDbPylotConfig::default(),
        }
    }

    pub fn with_runner(mut self, runner: Arc<dyn SqlRunner>) -> Self {
        self.runner = Some(runner);
        self
    }

    pub fn with_conversations(mut self, store: Arc<dyn ConversationStore>) -> Self {
        self.conversations = Some(store);
        self
    }

    pub fn with_config(mut self, config: OpenDbPylotConfig) -> Self {
        self.config = config;
        self
    }

    // --- Training (delegates to the vector store) ---

    pub async fn train_ddl(&self, ddl: &str) -> Result<()> {
        self.store.add_ddl(ddl).await
    }

    pub async fn train_documentation(&self, doc: &str) -> Result<()> {
        self.store.add_documentation(doc).await
    }

    pub async fn train_question_sql(&self, question: &str, sql: &str) -> Result<()> {
        self.store.add_question_sql(question, sql).await
    }

    // --- Introspection / direct access (used by the interactive CLI) ---

    /// Run raw SQL directly against the connected database.
    pub async fn run_sql(&self, sql: &str) -> Result<QueryResult> {
        match &self.runner {
            Some(runner) => runner.run_sql(sql).await,
            None => anyhow::bail!("no database connected"),
        }
    }

    pub async fn list_ddl(&self) -> Result<Vec<String>> {
        self.store.all_ddl().await
    }

    pub async fn list_documentation(&self) -> Result<Vec<String>> {
        self.store.all_documentation().await
    }

    pub async fn list_question_sql(&self) -> Result<Vec<crate::types::QuestionSql>> {
        self.store.all_question_sql().await
    }

    /// Generate SQL for a question. This is the RAG core:
    /// `generate_sql`: retrieve context, build the prompt, ask the LLM, extract SQL.
    ///
    /// If the model asks for an "intermediate_sql" exploratory query (guideline #2),
    /// and `allow_llm_to_see_data` is enabled, we run it, feed the results back, and
    /// ask again for the final SQL.
    pub async fn generate_sql(&self, question: &str) -> Result<String> {
        self.generate_sql_inner(question, &[]).await
    }

    /// Like `generate_sql`, but includes recent turns of the given conversation
    /// as context — enabling follow-up questions.
    pub async fn generate_sql_in_conversation(
        &self,
        conversation_id: &str,
        question: &str,
    ) -> Result<String> {
        let history = match &self.conversations {
            Some(store) => store.recent(conversation_id, self.config.history_limit).await?,
            None => Vec::new(),
        };
        self.generate_sql_inner(question, &history).await
    }

    /// The shared RAG core: retrieve context, build the prompt (with any
    /// conversation `history`), ask the LLM, handle the intermediate-SQL path,
    /// and extract the final SQL.
    async fn generate_sql_inner(&self, question: &str, history: &[QuestionSql]) -> Result<String> {
        let question_sql_list = self.store.get_similar_question_sql(question).await?;
        let ddl_list = self.store.get_related_ddl(question).await?;
        let mut doc_list = self.store.get_related_documentation(question).await?;

        let prompt = build_sql_prompt(
            &self.config.dialect,
            question,
            &ddl_list,
            &doc_list,
            &question_sql_list,
            history,
        );
        let response = self.llm.submit_prompt(prompt).await?;

        // Intermediate-SQL path: the model needs to peek at the data first.
        if response.contains("intermediate_sql") {
            if !self.config.allow_llm_to_see_data {
                return Ok(
                    "The LLM is not allowed to see the data in your database. This \
                     question requires inspecting column values. Enable \
                     allow_llm_to_see_data to proceed."
                        .to_string(),
                );
            }

            if let Some(runner) = &self.runner {
                let intermediate = extract_sql(&response);
                let df = runner.run_sql(&intermediate).await?;
                doc_list.push(format!(
                    "The following are the results of the intermediate SQL query {intermediate}:\n{}",
                    df.to_text()
                ));

                let prompt = build_sql_prompt(
                    &self.config.dialect,
                    question,
                    &ddl_list,
                    &doc_list,
                    &question_sql_list,
                    history,
                );
                let final_response = self.llm.submit_prompt(prompt).await?;
                return Ok(extract_sql(&final_response));
            }
        }

        Ok(extract_sql(&response))
    }

    /// Learn the database structure via the runner's `introspect_schema` method.
    /// Works for SQLite, PostgreSQL, and MySQL — each runner implements its own
    /// catalog query. Returns how many tables were learned.
    pub async fn train_from_schema(&self) -> Result<usize> {
        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no database runner configured"))?;
        let ddls = runner.introspect_schema().await?;
        // Replace, don't append: a re-import (or switching databases) should reflect
        // the current schema, not stack on top of a stale one.
        self.store.clear_ddl().await?;
        let count = ddls.len();
        for ddl in ddls {
            self.train_ddl(&ddl).await?;
        }
        // Also learn the distinct values of low-cardinality text columns, so the
        // model knows the real vocabulary (categories, statuses, product names…)
        // and can filter on values the user names. Stored as DDL so a re-import
        // replaces them (rather than duplicating) and user memories are untouched.
        if let Ok(hints) = runner.categorical_hints().await {
            for hint in hints {
                let _ = self.train_ddl(&hint).await;
            }
        }
        Ok(count)
    }

    /// Verify the database is actually reachable (a cheap `SELECT 1`). Distinguishes
    /// "configured" from "connected" so the UI can report the truth.
    pub async fn test_connection(&self) -> Result<()> {
        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no database connected"))?;
        runner.run_sql("SELECT 1").await.map(|_| ())
    }

    /// Kept for backwards compatibility — delegates to `train_from_schema`.
    pub async fn train_from_sqlite_schema(&self) -> Result<usize> {
        self.train_from_schema().await
    }

    /// Full `ask` flow: generate SQL, run it (if a DB is
    /// connected and the SQL is a safe read), and optionally self-train.
    pub async fn ask(&self, question: &str) -> Result<AskResult> {
        let sql = self.generate_sql(question).await?;

        let mut result = None;
        if let Some(runner) = &self.runner {
            if is_sql_valid(&sql) {
                let rows = runner.run_sql(&sql).await?;
                // Self-learning: remember successful question/SQL pairs.
                if self.config.auto_train && !rows.rows.is_empty() {
                    let _ = self.store.add_question_sql(question, &sql).await;
                }
                result = Some(rows);
            }
        }

        Ok(AskResult { sql, result })
    }

    /// Like `ask`, but conversation-aware: includes prior turns as context and
    /// records this turn so later follow-ups can reference it.
    pub async fn ask_in_conversation(&self, conversation_id: &str, question: &str) -> Result<AskResult> {
        let sql = self.generate_sql_in_conversation(conversation_id, question).await?;

        let mut result = None;
        if let Some(runner) = &self.runner {
            if is_sql_valid(&sql) {
                let rows = runner.run_sql(&sql).await?;
                if !rows.rows.is_empty() {
                    self.record_turn(conversation_id, question, &sql).await;
                }
                result = Some(rows);
            }
        }

        Ok(AskResult { sql, result })
    }

    /// Record a successful turn: append it to the conversation (for follow-ups)
    /// and, if `auto_train` is on, store it as a new training example (self-learning).
    pub async fn record_turn(&self, conversation_id: &str, question: &str, sql: &str) {
        if let Some(store) = &self.conversations {
            let _ = store
                .append(conversation_id, QuestionSql { question: question.to_string(), sql: sql.to_string() })
                .await;
        }
        if self.config.auto_train {
            let _ = self.store.add_question_sql(question, sql).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::local::LocalEmbedding;
    use crate::llm::mock::ScriptedMockLlm;
    use crate::sqlrunner::sqlite::SqliteRunner;
    use crate::vectorstore::memory::MemoryVectorStore;

    #[tokio::test]
    async fn intermediate_sql_runs_then_writes_final() {
        // Temp DB with a couple of rows.
        let path = std::env::temp_dir().join(format!("opendbpylot_itest_{}.db", std::process::id()));
        let db = SqliteRunner::new(path.to_string_lossy().to_string());
        db.run_sql("DROP TABLE IF EXISTS users").await.unwrap();
        db.run_sql("CREATE TABLE users (id INTEGER, country TEXT)").await.unwrap();
        db.run_sql("INSERT INTO users VALUES (1,'USA'),(2,'UK'),(3,'USA')")
            .await
            .unwrap();

        // The model first asks to peek (intermediate_sql), then writes the real query.
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "-- intermediate_sql\nSELECT DISTINCT country FROM users;".to_string(),
            "```sql\nSELECT COUNT(*) FROM users WHERE country = 'USA';\n```".to_string(),
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));

        let opendbpylot = OpenDbPylot::new(llm, store).with_runner(Arc::new(db)).with_config(
            OpenDbPylotConfig {
                dialect: "SQLite".into(),
                auto_train: false,
                allow_llm_to_see_data: true,
                history_limit: 5,
            },
        );

        let sql = opendbpylot.generate_sql("how many USA users?").await.unwrap();
        assert_eq!(sql, "SELECT COUNT(*) FROM users WHERE country = 'USA';");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn conversation_history_is_recorded_and_reused() {
        use crate::conversation::{ConversationStore, MemoryConversationStore};
        use crate::llm::mock::MockLlm;

        let convos = Arc::new(MemoryConversationStore::new());
        let llm = Arc::new(MockLlm::with_default_sql());
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let opendbpylot = OpenDbPylot::new(llm, store).with_conversations(convos.clone());

        // No runner, so this records the turn manually to verify history plumbing.
        opendbpylot.record_turn("c1", "list users", "SELECT * FROM users;").await;
        let recent = convos.recent("c1", 5).await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].question, "list users");

        // A second conversation is isolated.
        assert!(convos.recent("c2", 5).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn intermediate_sql_blocked_without_permission() {
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "-- intermediate_sql\nSELECT DISTINCT country FROM users;".to_string(),
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let opendbpylot = OpenDbPylot::new(llm, store); // default: allow_llm_to_see_data = false

        let sql = opendbpylot.generate_sql("how many USA users?").await.unwrap();
        assert!(sql.contains("not allowed to see the data"));
    }
}
