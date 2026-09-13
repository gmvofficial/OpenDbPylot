//! # opendbpylot
//!
//! Turn a natural-language question into SQL using **Retrieval-Augmented Generation (RAG)**.
//!
//! ## The pipeline
//! ```text
//! question
//!   -> retrieve relevant context (similar Q/SQL, related DDL, related docs)   [vectorstore]
//!   -> build a prompt out of that context                                     [prompt]
//!   -> ask the LLM to write SQL                                               [llm]
//!   -> clean the SQL out of the reply                                         [sql]
//!   -> run it on the database                                                 [sqlrunner]
//!   -> return rows
//! ```
//!
//! Every layer is a **trait** so the concrete provider (OpenAI vs mock, SQLite vs
//! Postgres, in-memory vs hosted vector store) can be swapped without touching the
//! orchestration logic — an "interface + plug-in" design throughout.

pub mod llm;
pub mod core;
pub mod capabilities;
pub mod tools;
pub mod embedding;
pub mod eval;
pub mod conversation;
pub mod vectorstore;
pub mod sqlrunner;
pub mod prompt;
pub mod retrieval;
pub mod schema;
pub mod sql;
pub mod types;
pub mod opendbpylot;
pub mod demo;
pub mod secret;
pub mod settings;
pub mod app;
pub mod server;
pub mod cli;
pub mod mcp;
