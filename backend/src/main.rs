//! Process entry: load config, install the tracing subscriber, open the
//! database, wire the trait seams, and serve the Axum app.

use std::net::SocketAddr;

use second_brain_backend::{
    auth,
    config::{Config, LogFormat},
    db::Db,
    embedding::FakeEmbedding,
    extractor::FakeExtractor,
    llm::FakeLlm,
    routes,
    state::AppState,
};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env();
    init_tracing(&config);
    tracing::info!(bind = %config.bind_addr, "starting second-brain backend");

    let db = Db::open(&config.database_url)?;
    let webauthn = auth::build_webauthn(
        &config.webauthn_rp_id,
        &config.webauthn_rp_origin,
        &config.webauthn_rp_name,
    )?;
    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        llm: Arc::new(FakeLlm),
        embedding: Arc::new(FakeEmbedding { dim: 1024 }),
        extractor: Arc::new(FakeExtractor),
        auth: auth::AuthService::new(webauthn),
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

fn init_tracing(config: &Config) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.rust_log));
    match config.log_format {
        LogFormat::Json => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        LogFormat::Plain => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
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
