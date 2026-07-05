//! Second Brain backend — the Rust/Axum orchestrator and graph engine.
//!
//! Module layout is intentionally seam-first (per the skeleton issue):
//! the HTTP layer, DB access, and the LLM/embedding clients are behind traits
//! so later slices plug in real implementations without rewiring call sites.

pub mod auth;
pub mod braindump;
pub mod chat;
pub mod chat_inference;
pub mod config;
pub mod db;
pub mod delta;
pub mod embedding;
pub mod error;
pub mod extractor;
pub mod gemini;
pub mod graph;
pub mod graph_repo;
pub mod llm;
pub mod logs;
pub mod ontology;
pub mod retrieval;
pub mod routes;
pub mod snapshot;
pub mod state;
pub mod thematic;

pub use db::Db;
