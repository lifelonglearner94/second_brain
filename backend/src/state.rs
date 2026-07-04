//! Shared application state passed to every Axum handler via `State`.

use std::sync::Arc;

use crate::auth::AuthService;
use crate::config::Config;
use crate::db::Db;
use crate::embedding::{EmbeddingClient, FakeEmbedding};
use crate::extractor::Extractor;
use crate::llm::LlmClient;
use crate::logs::LogBuffer;
use crate::ontology::RefactorRunner;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub llm: Arc<dyn LlmClient>,
    pub embedding: Arc<dyn EmbeddingClient>,
    pub extractor: Arc<dyn Extractor>,
    pub auth: AuthService,
    pub log_buffer: LogBuffer,
    pub refactor_runner: RefactorRunner,
}

impl AppState {
    /// Construct test state with the fake LLM/embedding/extractor clients and a
    /// WebAuthn instance matching the test config (`localhost`). The vec0
    /// embedding tables are created at the fake embedding's dimensionality so
    /// the accretion pipeline can store/retrieve embeddings in tests.
    pub fn for_tests(db: Db) -> Self {
        let config = Config::for_tests();
        let webauthn = crate::auth::build_webauthn(
            &config.webauthn_rp_id,
            &config.webauthn_rp_origin,
            &config.webauthn_rp_name,
        )
        .expect("test webauthn config must be valid");
        let embedding = Arc::new(FakeEmbedding::default());
        db.ensure_vec_tables(embedding.dim())
            .expect("vec tables for tests");
        Self {
            db,
            config: Arc::new(config),
            llm: Arc::new(crate::llm::FakeLlm),
            embedding,
            extractor: Arc::new(crate::extractor::FakeExtractor),
            auth: AuthService::new(webauthn),
            log_buffer: LogBuffer::with_default_capacity(),
            refactor_runner: RefactorRunner::new(),
        }
    }
}
