//! Process entry: load config, install the tracing subscriber, open the
//! database, wire the trait seams, and serve the Axum app.

use std::net::SocketAddr;
use std::sync::Arc;

use second_brain_backend::{
    auth, braindump,
    config::{Config, LogFormat},
    db::{self, Db, BOOTSTRAP_ADMIN_USER_ID},
    fallback::FallbackLlm,
    gemini::GeminiClient,
    graph_repo::{GraphRepo, SqliteGraphRepo},
    llm::{FakeLlm, Llm},
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

    // Wire the real Gemini seam when an API key is present; otherwise fall
    // back to the fake so a dev/CI box without a key still runs (the ingest
    // pipeline is exercised end-to-end with stub extraction/embedding). The
    // single LLM seam (clean, generate_pinned, synthesize, extract, embed) -
    // issue #39 collapsed the former three traits into one.
    //
    // Issue #86: the primary GeminiClient is wrapped in a circuit-breaker
    // `FallbackLlm` so that, after 5 consecutive transient (429/5xx/transport)
    // text-generation failures, subsequent text calls route to a fallback
    // model (default `gemini-3.1-flash-lite`) until a cooldown expires and a
    // half-open probe of the primary succeeds. Embeddings bypass the circuit
    // (identity-calibrated, ADR-0001). The fallback client shares the
    // primary's credentials/embed config/HTTP pool with only the text model
    // swapped. ADR-0003's pinned-model determinism is an accepted, logged
    // exception on the free tier - see the ADR's free-tier-fallback addendum.
    let llm: Arc<dyn Llm> = match GeminiClient::from_env()? {
        Some(primary) => {
            let fallback = primary.fallback();
            tracing::info!(
                fallback_model = %fallback.text_model_name(),
                "gemini seam wired (primary + fallback circuit breaker)"
            );
            Arc::new(FallbackLlm::from_env(Arc::new(primary), Arc::new(fallback)))
        }
        None => {
            tracing::warn!(
                "GEMINI_API_KEY unset - falling back to fake LLM. \
                 Set it to run real extraction."
            );
            Arc::new(FakeLlm { dim: 1024 })
        }
    };

    // The vec0 embedding tables are dim-dependent (the embedding model fixes
    // the dimensionality), so they are created here at the live client's dim
    // rather than in the dim-agnostic schema migration.
    db.ensure_vec_tables(llm.dim())?;

    // Seed type-embeddings for any ontology types missing one (the day-zero
    // vocabulary has no embeddings until the first run - ADR-0003 dedup needs
    // them to auto-merge duplicate proposals). Issue #72: the ontology is
    // per-user; the seed runs for the bootstrap admin at startup. Each new
    // user's vocabulary is seeded on first activity via
    // `db::seed_ontology_for_user`.
    let _ = db::seed_ontology_for_user(&db, BOOTSTRAP_ADMIN_USER_ID).await;
    let seeded = second_brain_backend::ontology::seed_type_embeddings(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        llm.as_ref(),
    )
    .await?;
    if seeded > 0 {
        tracing::info!(count = seeded, "seeded type embeddings for ontology dedup");
    }

    let graph_repo: Arc<dyn GraphRepo> = Arc::new(SqliteGraphRepo::new(db.clone()));
    let ingest_runner = braindump::IngestRunner::new();
    // Issue #84: startup recovery scan. Any braindump left `pending`
    // (mid-processing, or awaiting retry after a transient Gemini failure -
    // issue #85) is re-spawned so a restart does not strand it. Runs before
    // serving so the resumed pipelines are in flight by the time requests
    // arrive; the spawned tasks commit out-of-band.
    let resumed = braindump::recover_pending(&db, &llm, &Arc::new(config.clone()), &ingest_runner)
        .await
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "ingest recovery scan failed");
            0
        });
    if resumed > 0 {
        tracing::info!(count = resumed, "resumed pending braindump ingests");
    }
    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        llm,
        auth: auth::AuthService::new(webauthn),
        log_buffer,
        refactor_runner: RefactorRunner::new(),
        ingest_runner,
        graph_repo,
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
