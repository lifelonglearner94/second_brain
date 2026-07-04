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
    logs::LogBuffer,
    routes,
    state::AppState,
};
use std::sync::Arc;

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
    let state = AppState {
        db,
        config: Arc::new(config.clone()),
        llm: Arc::new(FakeLlm),
        embedding: Arc::new(FakeEmbedding { dim: 1024 }),
        extractor: Arc::new(FakeExtractor),
        auth: auth::AuthService::new(webauthn),
        log_buffer,
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
