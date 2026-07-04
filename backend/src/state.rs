//! Shared application state passed to every Axum handler via `State`.

use std::sync::Arc;

use crate::config::Config;
use crate::db::Db;
use crate::embedding::EmbeddingClient;
use crate::llm::LlmClient;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub llm: Arc<dyn LlmClient>,
    pub embedding: Arc<dyn EmbeddingClient>,
}

impl AppState {
    /// Construct test state with the fake LLM/embedding clients.
    pub fn for_tests(db: Db) -> Self {
        Self {
            db,
            config: Arc::new(Config::for_tests()),
            llm: Arc::new(crate::llm::FakeLlm),
            embedding: Arc::new(crate::embedding::FakeEmbedding::default()),
        }
    }
}
