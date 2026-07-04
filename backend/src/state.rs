//! Shared application state passed to every Axum handler via `State`.

use std::sync::Arc;

use crate::auth::AuthService;
use crate::config::Config;
use crate::db::Db;
use crate::embedding::EmbeddingClient;
use crate::llm::LlmClient;
use crate::logs::LogBuffer;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub llm: Arc<dyn LlmClient>,
    pub embedding: Arc<dyn EmbeddingClient>,
    pub auth: AuthService,
    pub log_buffer: LogBuffer,
}

impl AppState {
    /// Construct test state with the fake LLM/embedding clients and a WebAuthn
    /// instance matching the test config (`localhost`).
    pub fn for_tests(db: Db) -> Self {
        let config = Config::for_tests();
        let webauthn = crate::auth::build_webauthn(
            &config.webauthn_rp_id,
            &config.webauthn_rp_origin,
            &config.webauthn_rp_name,
        )
        .expect("test webauthn config must be valid");
        Self {
            db,
            config: Arc::new(config),
            llm: Arc::new(crate::llm::FakeLlm),
            embedding: Arc::new(crate::embedding::FakeEmbedding::default()),
            auth: AuthService::new(webauthn),
            log_buffer: LogBuffer::with_default_capacity(),
        }
    }
}
