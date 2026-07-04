//! Process entry: load config, install the tracing subscriber, open the
//! database, wire the trait seams, and serve the Axum app.

use std::net::SocketAddr;
use std::sync::Arc;

use second_brain_backend::{
    auth,
    config::{Config, LogFormat},
    db::Db,
    embedding::{EmbeddingClient, FakeEmbedding},
    extractor::Extractor,
    gemini::GeminiClient,
    llm::LlmClient,
    logs::LogBuffer,
    ontology::RefactorRunner,
    routes,
    state::AppState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env();
    let log_buffer = LogBuffer::with_default_capacity();
    init_tracing(&config, log_buffer.clone());
    tracing::info!(bind = %config.bind_addr, "starting second-brain backend");

    let db = Db::open(&config.database_url)?;
    let webauthn = auth::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_origin,
        &config.webauthn_rp_name,
    )?;

    // Wire the real Gemini seams when an API key is present; otherwise fall
    // back to the fakes so a dev/CI box without a key still runs (the ingest
    // pipeline is exercised end-to-end with stub extraction/embedding). One
    // GeminiClient implements all three seams (clean, extract, embed).
    let (llm, extractor, embedding): (
        Arc<dyn LlmClient>,
        Arc<dyn Extractor>,
        Arc<dyn EmbeddingClient + Send + Sync>,
    ) = match GeminiClient::from_env() {
        Some(gemini) => {
            tracing::info!("gemini seams wired (real Gemini LLM + extractor + embeddings)");
            let llm: Arc<dyn LlmClient> = Arc::new(gemini.clone());
            let extractor: Arc<dyn Extractor> = Arc::new(gemini.clone());
            let embedding: Arc<dyn EmbeddingClient + Send + Sync> = Arc::new(gemini);
            (llm, extractor, embedding)
        }
        None => {
            tracing::warn!(
                "GEMINI_API_KEY unset — falling back to fake LLM/extractor/embedding. \
                 Set it to run real extraction."
            );
            (
                Arc::new(second_brain_backend::llm::FakeLlm),
                Arc::new(second_brain_backend::extractor::FakeExtractor),
                Arc::new(FakeEmbedding { dim: 1024 }),
            )
        }
    };

    // The vec0 embedding tables are dim-dependent (the embedding model fixes
    // the dimensionality), so they are created here at the live client's dim
    // rather than in the dim-agnostic schema migration.
    db.ensure_vec_tables(embedding.dim())?;

    // Seed type-embeddings for any ontology types missing one (the day-zero
    // vocabulary has no embeddings until the first run — ADR-0003 dedup needs
    // them to auto-merge duplicate proposals). Idempotent: already-embedded
    // types are skipped.
    let seeded =
        second_brain_backend::ontology::seed_type_embeddings(&db, embedding.as_ref()).await?;
    if seeded > 0 {
        tracing::info!(count = seeded, "seeded type embeddings for ontology dedup");
    }

    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        llm,
        extractor,
        embedding,
        auth: auth::AuthService::new(webauthn),
        log_buffer,
        refactor_runner: RefactorRunner::new(),
    };
    let app = routes::router(state);

    let addr: SocketAddr = config.bind_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn init_tracing(config: &Config, log_buffer: LogBuffer) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.rust_log));
    let buffer_layer = second_brain_backend::logs::LogBufferLayer::new(log_buffer);
    match config.log_format {
        LogFormat::Json => {
            let fmt = tracing_subscriber::fmt::layer().json();
            tracing_subscriber::registry()
                .with(filter)
                .with(buffer_layer)
                .with(fmt)
                .init();
        }
        LogFormat::Plain => {
            let fmt = tracing_subscriber::fmt::layer();
            tracing_subscriber::registry()
                .with(filter)
                .with(buffer_layer)
                .with(fmt)
                .init();
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install terminate signal")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
