//! The orchestrator — the central engine that ties the pipeline together.
//!
//! It wires together the four swappable pieces (LLM, vector store, SQL runner)
//! and exposes the two methods that matter: `train` and `ask`.

use std::sync::Arc;

use anyhow::Result;

use tokio::sync::OnceCell;

use crate::conversation::ConversationStore;
use crate::llm::LlmService;
use crate::prompt::{build_repair_prompt, build_sql_prompt};
use crate::schema::SchemaIndex;
use crate::sql::{extract_sql, is_sql_valid};
use crate::sqlrunner::{QueryResult, SqlRunner};
use crate::types::QuestionSql;
use crate::vectorstore::VectorStore;

/// Tunable engine settings.
pub struct OpenDbPylotConfig {
    /// SQL dialect named in the prompt (e.g. "SQLite", "PostgreSQL").
    pub dialect: String,
    /// After a successful question, capture it for review as a possible new
    /// training example.
    ///
    /// Captures go to the review queue, not straight into retrieval — see
    /// [`crate::review`]. "The query returned rows" is not "the query was
    /// right", and an unreviewed example teaches its mistake to every later
    /// question that retrieves it.
    pub auto_train: bool,
    /// Where captured pairs wait for review. `None` disables capture entirely.
    pub review_queue_path: Option<std::path::PathBuf>,
    /// Allow the LLM to run an exploratory ("intermediate") query and see its
    /// results before writing the final SQL. Off by default for privacy.
    pub allow_llm_to_see_data: bool,
    /// How many prior conversation turns to include as context for follow-ups.
    pub history_limit: usize,
    /// How many times a failing generated query may be sent back to the LLM
    /// (with the error message) for correction before giving up.
    /// 0 disables self-repair.
    pub max_sql_repairs: usize,
    /// Approximate token budget for the assembled prompt. Context sections are
    /// added highest-priority-first (DDL → docs → examples → history) and the
    /// rest is dropped, so a large schema can't overflow the model's context.
    pub max_prompt_tokens: usize,
    /// Rerank retrieved DDL with schema structure before it reaches the
    /// prompt. Retrieval fetches `rerank_pool` candidates and this cuts them
    /// back to what the prompt budget expects.
    ///
    /// Off means the fused order is used as-is.
    pub rerank_ddl: bool,
    /// How many DDL candidates to retrieve before reranking. Ignored when
    /// `rerank_ddl` is off.
    pub rerank_pool: usize,
    /// When true, `ask` makes one extra bounded LLM call to produce a short
    /// natural-language answer alongside the rows (`AskResult::answer`).
    /// Off by default — it doubles LLM calls, so callers opt in.
    pub summarize_results: bool,
}

impl Default for OpenDbPylotConfig {
    fn default() -> Self {
        Self {
            dialect: "SQLite".into(),
            // Off by default: blanket-saving every answered question as "training"
            // pollutes the knowledge base with whatever SQL the model happened to
            // produce. Saving examples should be a deliberate action.
            auto_train: false,
            review_queue_path: None,
            allow_llm_to_see_data: false,
            history_limit: 5,
            max_sql_repairs: 2,
            max_prompt_tokens: crate::prompt::DEFAULT_MAX_PROMPT_TOKENS,
            // Measured off. On the demo schema (four tables, all of which fit
            // the prompt budget) reranking can only reorder, and a single
            // benchmark run came out 10 points *worse* than the fused order.
            // It stays available because table selection is the dominant
            // failure mode on a large schema, where retrieval must actually
            // choose — but it is not turned on by a claim, only by a
            // measurement on the schema in question.
            rerank_ddl: false,
            rerank_pool: 24,
            summarize_results: false,
        }
    }
}

/// What `ask` returns: the generated SQL and (if a runner is set) the rows.
#[derive(Debug)]
pub struct AskResult {
    pub sql: String,
    pub result: Option<QueryResult>,
    /// How many self-repair round-trips were needed (0 = first attempt worked).
    pub repairs_used: usize,
    /// A short natural-language answer, present only when `summarize_results` is
    /// enabled and the query returned rows. Best-effort — `None` on any failure.
    pub answer: Option<String>,
}

pub struct OpenDbPylot {
    llm: Arc<dyn LlmService>,
    store: Arc<dyn VectorStore>,
    runner: Option<Arc<dyn SqlRunner>>,
    conversations: Option<Arc<dyn ConversationStore>>,
    config: OpenDbPylotConfig,
    /// Lazily introspected database schema, used for pre-execution validation.
    /// Cached for the lifetime of this instance (instances are rebuilt on
    /// reconnect/settings changes).
    schema_index: OnceCell<SchemaIndex>,
}

impl OpenDbPylot {
    pub fn new(llm: Arc<dyn LlmService>, store: Arc<dyn VectorStore>) -> Self {
        Self {
            llm,
            store,
            runner: None,
            conversations: None,
            config: OpenDbPylotConfig::default(),
            schema_index: OnceCell::new(),
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
        let ddl_list = self.related_ddl_reranked(question).await?;
        let mut doc_list = self.store.get_related_documentation(question).await?;
        tracing::debug!(
            ddl = ddl_list.len(),
            docs = doc_list.len(),
            examples = question_sql_list.len(),
            history = history.len(),
            "retrieved context"
        );

        let prompt = build_sql_prompt(
            &self.config.dialect,
            question,
            &ddl_list,
            &doc_list,
            &question_sql_list,
            history,
            self.config.max_prompt_tokens,
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
                    self.config.max_prompt_tokens,
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

    /// Full `ask` flow: generate SQL, validate + run it (if a DB is connected and
    /// the SQL is a safe read), self-repairing failures, and — when enabled —
    /// capture the pair for review.
    ///
    /// Capture is not training. The pair waits in the review queue until a human
    /// approves it, because "the query returned rows" is not "the query was
    /// right", and a wrong exemplar teaches its mistake to everything that
    /// later retrieves it.
    pub async fn ask(&self, question: &str) -> Result<AskResult> {
        let mut out = self.ask_core(question, &[]).await?;
        if let Some(rows) = &out.result {
            if self.config.auto_train && !rows.rows.is_empty() {
                self.capture_for_review(question, &out.sql, rows.rows.len()).await;
            }
        }
        self.maybe_summarize(question, &mut out).await;
        Ok(out)
    }

    /// Like `ask`, but conversation-aware: includes prior turns as context and
    /// records this turn so later follow-ups can reference it.
    pub async fn ask_in_conversation(&self, conversation_id: &str, question: &str) -> Result<AskResult> {
        let history = match &self.conversations {
            Some(store) => store.recent(conversation_id, self.config.history_limit).await?,
            None => Vec::new(),
        };
        let mut out = self.ask_core(question, &history).await?;
        if let Some(rows) = &out.result {
            if !rows.rows.is_empty() {
                self.record_turn_with_rows(conversation_id, question, &out.sql, rows.rows.len())
                    .await;
            }
        }
        self.maybe_summarize(question, &mut out).await;
        Ok(out)
    }

    /// Fill `out.answer` with a short natural-language takeaway when
    /// `summarize_results` is on and there are rows. Best-effort: a failed or
    /// empty summary leaves `answer = None` and never affects the query result.
    async fn maybe_summarize(&self, question: &str, out: &mut AskResult) {
        if !self.config.summarize_results {
            return;
        }
        let Some(rows) = &out.result else { return };
        if rows.rows.is_empty() {
            return;
        }
        // Bound the tokens: summarize from at most the first 20 rows.
        let preview = QueryResult {
            columns: rows.columns.clone(),
            rows: rows.rows.iter().take(20).cloned().collect(),
        };
        let prompt = crate::prompt::build_summary_prompt(question, &out.sql, &preview.to_text());
        match self.llm.submit_prompt(prompt).await {
            Ok(text) if !text.trim().is_empty() => {
                out.answer = Some(text.trim().to_string());
            }
            Ok(_) => {}
            Err(e) => tracing::debug!(error = %e, "result summary failed (ignored)"),
        }
    }

    /// The shared ask path with the **self-repair loop**:
    ///
    /// ```text
    /// generate → schema-validate ──issues──▶ repair prompt ─▶ regenerate ─▶ (loop)
    ///                │ ok                          ▲
    ///                ▼                             │ error
    ///             execute ──────────────────────────
    ///                │ ok
    ///                ▼
    ///              rows
    /// ```
    ///
    /// Schema validation catches hallucinated tables/columns *before* touching
    /// the database; execution errors are fed back verbatim. After
    /// `max_sql_repairs` failed corrections the last error is returned honestly —
    /// never a silently-broken result.
    async fn ask_core(&self, question: &str, history: &[QuestionSql]) -> Result<AskResult> {
        let mut sql = self.generate_sql_inner(question, history).await?;
        let mut repairs_used = 0usize;
        tracing::debug!(%sql, "generated sql");

        let Some(runner) = self.runner.as_ref() else {
            return Ok(AskResult { sql, result: None, repairs_used, answer: None });
        };

        loop {
            // Not a runnable read (e.g. the model replied with an explanation
            // instead of SQL) — return it as-is for the caller to display.
            if !is_sql_valid(&sql) {
                tracing::debug!(%sql, "not a runnable read; returning as-is");
                return Ok(AskResult { sql, result: None, repairs_used, answer: None });
            }

            // Cheap static check first: hallucinated tables/columns never reach
            // the database. Falls back to execution when it can't be sure.
            let issues = self.schema().await.validate(&sql, &self.config.dialect);
            let error = if !issues.is_empty() {
                issues.join("; ")
            } else {
                match runner.run_sql(&sql).await {
                    Ok(rows) => {
                        tracing::debug!(rows = rows.rows.len(), repairs = repairs_used, "query succeeded");
                        return Ok(AskResult { sql, result: Some(rows), repairs_used, answer: None });
                    }
                    Err(e) => format!("{e:#}"),
                }
            };
            tracing::info!(attempt = repairs_used, %error, "query failed; attempting repair");

            if repairs_used >= self.config.max_sql_repairs {
                anyhow::bail!(
                    "SQL still failing after {repairs_used} repair attempt(s).\n\
                     Last error: {error}\nSQL: {sql}"
                );
            }
            repairs_used += 1;

            let ddl_list = self.related_ddl_reranked(question).await.unwrap_or_default();
            let prompt = build_repair_prompt(
                &self.config.dialect,
                question,
                &sql,
                &error,
                &ddl_list,
                self.config.max_prompt_tokens,
            );
            let response = self.llm.submit_prompt(prompt).await?;
            sql = extract_sql(&response);
        }
    }

    /// Retrieve DDL and, when enabled, rerank it with schema structure.
    ///
    /// Picking the wrong table is the dominant failure mode in text-to-SQL, and
    /// flat-text fusion scores a table name and its fortieth column the same.
    /// See [`crate::rerank`].
    async fn related_ddl_reranked(&self, question: &str) -> Result<Vec<String>> {
        let candidates = self.store.get_related_ddl(question).await?;
        if !self.config.rerank_ddl {
            return Ok(candidates);
        }
        // The store's own limit caps what comes back, so the pool is an upper
        // bound rather than a guarantee — reranking a short list is harmless.
        let keep = candidates.len().min(self.config.rerank_pool / 2).max(1);
        Ok(crate::rerank::rerank(question, candidates, keep.max(8)))
    }

    /// Lazily introspect the connected database into a [`SchemaIndex`], cached
    /// for this instance's lifetime. Unavailable introspection (no runner, or a
    /// backend without it) yields an empty index — validation becomes a no-op.
    async fn schema(&self) -> &SchemaIndex {
        self.schema_index
            .get_or_init(|| async {
                match &self.runner {
                    Some(runner) => match runner.introspect_schema().await {
                        Ok(ddl) => SchemaIndex::from_ddl(&ddl, &self.config.dialect),
                        Err(_) => SchemaIndex::empty(),
                    },
                    None => SchemaIndex::empty(),
                }
            })
            .await
    }

    /// Record a successful turn: append it to the conversation (for follow-ups)
    /// and, if `auto_train` is on, store it as a new training example (self-learning).
    pub async fn record_turn(&self, conversation_id: &str, question: &str, sql: &str) {
        self.record_turn_with_rows(conversation_id, question, sql, 0).await;
    }

    /// As [`record_turn`](Self::record_turn), with the row count carried into
    /// the review queue — zero rows is a common sign of a subtly wrong query,
    /// and a reviewer wants to see it.
    pub async fn record_turn_with_rows(
        &self,
        conversation_id: &str,
        question: &str,
        sql: &str,
        row_count: usize,
    ) {
        if let Some(store) = &self.conversations {
            let _ = store
                .append(conversation_id, QuestionSql { question: question.to_string(), sql: sql.to_string() })
                .await;
        }
        if self.config.auto_train {
            self.capture_for_review(question, sql, row_count).await;
        }
    }

    /// Queue a question/SQL pair for human review.
    ///
    /// Best-effort: a queue that cannot be written must never fail the user's
    /// query, which already succeeded.
    pub async fn capture_for_review(&self, question: &str, sql: &str, row_count: usize) {
        let Some(path) = &self.config.review_queue_path else {
            return;
        };
        let mut queue = match crate::review::ReviewQueue::load(path) {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!("Could not load the review queue: {e:#}");
                return;
            }
        };
        if queue.capture(question, sql, row_count) == crate::review::Captured::Queued {
            if let Err(e) = queue.save(path) {
                tracing::warn!("Could not save the review queue: {e:#}");
            }
        }
    }

    /// Add an approved pair to the training corpus.
    ///
    /// The only path from the review queue into retrieval.
    pub async fn train_approved(&self, pair: &QuestionSql) -> Result<()> {
        self.store.add_question_sql(&pair.question, &pair.sql).await
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
                ..Default::default()
            },
        );

        let sql = opendbpylot.generate_sql("how many USA users?").await.unwrap();
        assert_eq!(sql, "SELECT COUNT(*) FROM users WHERE country = 'USA';");

        let _ = std::fs::remove_file(&path);
    }

    /// Fresh temp SQLite DB with a small `users` table.
    async fn temp_users_db(tag: &str) -> (SqliteRunner, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "opendbpylot_{tag}_{}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let db = SqliteRunner::new(path.to_string_lossy().to_string());
        db.run_sql("CREATE TABLE users (id INTEGER, name TEXT, country TEXT)").await.unwrap();
        db.run_sql("INSERT INTO users VALUES (1,'Ana','USA'),(2,'Bo','UK')").await.unwrap();
        (db, path)
    }

    #[tokio::test]
    async fn ask_repairs_hallucinated_column_before_touching_the_db() {
        let (db, path) = temp_users_db("repair1").await;

        // First reply has a hallucinated column; the repair reply is correct.
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "SELECT nam FROM users;".to_string(),
            "SELECT name FROM users;".to_string(),
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(llm, store).with_runner(Arc::new(db));

        let out = bot.ask("what are the user names?").await.unwrap();
        assert_eq!(out.repairs_used, 1);
        assert_eq!(out.sql, "SELECT name FROM users;");
        assert_eq!(out.result.unwrap().rows.len(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn ask_repairs_execution_errors_too() {
        let (db, path) = temp_users_db("repair2").await;

        // First reply references a missing table via SQL that *parses* oddly
        // enough to reach execution (unknown function) — the DB error comes back,
        // the repair reply fixes it.
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "SELECT no_such_function(name) FROM users;".to_string(),
            "SELECT name FROM users;".to_string(),
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(llm, store).with_runner(Arc::new(db));

        let out = bot.ask("names?").await.unwrap();
        assert_eq!(out.repairs_used, 1);
        assert!(out.result.is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn ask_gives_up_honestly_after_max_repairs() {
        let (db, path) = temp_users_db("repair3").await;

        // Every reply is broken — the loop must terminate with an error, not hang
        // or return a silently-broken result.
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "SELECT nam FROM users;".to_string(), // repeats forever
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(llm, store).with_runner(Arc::new(db));

        let err = bot.ask("names?").await.unwrap_err().to_string();
        assert!(err.contains("2 repair attempt(s)"), "unexpected error: {err}");
        assert!(err.contains("nam"), "error should carry the failing detail: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn repair_disabled_when_max_is_zero() {
        let (db, path) = temp_users_db("repair0").await;

        let llm = Arc::new(ScriptedMockLlm::new(vec!["SELECT nam FROM users;".to_string()]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(llm, store)
            .with_runner(Arc::new(db))
            .with_config(OpenDbPylotConfig { max_sql_repairs: 0, ..Default::default() });

        let err = bot.ask("names?").await.unwrap_err().to_string();
        assert!(err.contains("0 repair attempt(s)"), "unexpected error: {err}");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn summary_is_produced_when_enabled_and_omitted_otherwise() {
        use crate::llm::mock::ScriptedMockLlm;

        let (db, path) = temp_users_db("summary").await;
        let db = Arc::new(db);

        // The mock returns the SQL first (for generate_sql), then the summary text
        // on the second submit_prompt call (maybe_summarize).
        let llm = Arc::new(ScriptedMockLlm::new(vec![
            "SELECT name FROM users;".to_string(),
            "There are two users.".to_string(),
        ]));
        let store = Arc::new(MemoryVectorStore::new(Arc::new(LocalEmbedding::new())));
        let bot = OpenDbPylot::new(llm, store.clone())
            .with_runner(db.clone())
            .with_config(OpenDbPylotConfig { summarize_results: true, ..Default::default() });

        let out = bot.ask("who are the users?").await.unwrap();
        assert!(out.result.is_some());
        assert_eq!(out.answer.as_deref(), Some("There are two users."));

        // With the flag off, no summary call is made and answer stays None.
        let llm2 = Arc::new(ScriptedMockLlm::new(vec!["SELECT name FROM users;".to_string()]));
        let bot2 = OpenDbPylot::new(llm2, store).with_runner(db); // default: summarize off
        let out2 = bot2.ask("who are the users?").await.unwrap();
        assert!(out2.answer.is_none());

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
