use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{
    connect_async, tungstenite::handshake::client::generate_key,
    tungstenite::protocol::Message as TungsteniteMessage,
};
use tracing::{error, info, warn};

use crate::{error::Error, state::AppState};

/// KeepAlive interval (Deepgram docs: send every 3–5 s to avoid the 10-second
/// NET-0001 timeout during silence). Overridable via `DEEPGRAM_KEEPALIVE_SECS`
/// so tests can use a short interval without waiting.
fn keepalive_interval() -> Duration {
    std::env::var("DEEPGRAM_KEEPALIVE_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(4))
}

/// The Deepgram streaming WebSocket URL with query parameters matching the
/// frontend's audio format (Int16 PCM, 16 kHz, mono). Overridden by
/// `DEEPGRAM_WS_URL` in tests to point at a mock server.
fn deepgram_ws_url() -> String {
    std::env::var("DEEPGRAM_WS_URL").unwrap_or_else(|_| {
        "wss://api.deepgram.com/v1/listen?model=nova-3&language=de&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true"
            .to_string()
    })
}

const KEEPALIVE_MSG: &str = r#"{"type":"KeepAlive"}"#;
const CLOSE_STREAM_MSG: &str = r#"{"type":"CloseStream"}"#;

/// WebSocket proxy route for Deepgram STT.
/// Relays binary PCM audio from client to Deepgram and text JSON transcripts
/// back, keeping the Deepgram API key server-side (ADR-0004).
pub async fn deepgram_proxy(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
) -> Result<impl IntoResponse, Error> {
    let api_key = std::env::var("DEEPGRAM_API_KEY").map_err(|_| {
        warn!("DEEPGRAM_API_KEY not set, refusing WebSocket upgrade");
        Error::ServiceUnavailable("Deepgram API key not configured".to_string())
    })?;
    let url = deepgram_ws_url();
    Ok(ws.on_upgrade(move |socket| handle_socket(socket, url, api_key)))
}

/// Bidirectional relay between the client WebSocket and Deepgram's streaming
/// API. Binary PCM frames go up, transcript JSON comes down. A KeepAlive task
/// pings Deepgram on a timer so the connection survives silence, and a
/// `CloseStream` message is sent on teardown to flush remaining results.
async fn handle_socket(mut client_ws: WebSocket, deepgram_url: String, api_key: String) {
    info!("Client connected to Deepgram proxy");

    // Build the upgrade request with the Authorization header and the Host
    // derived from the URL. `connect_async` adds the remaining WebSocket
    // upgrade headers (Sec-WebSocket-*, Upgrade, Connection) automatically,
    // but NOT the Host header — that must be set explicitly when passing a
    // pre-built `Request` (as opposed to a URL string).
    let uri: http::Uri = match deepgram_url.parse() {
        Ok(u) => u,
        Err(e) => {
            error!("Invalid Deepgram URL: {e}");
            let _ = client_ws.close().await;
            return;
        }
    };
    let host = uri
        .authority()
        .map(|a| a.as_str())
        .unwrap_or("api.deepgram.com");

    let request = match tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&deepgram_url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Host", host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .body(())
    {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to build Deepgram request: {e}");
            let _ = client_ws.close().await;
            return;
        }
    };

    let (deepgram_ws, _) = match connect_async(request).await {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to connect to Deepgram: {e}");
            let _ = client_ws.close().await;
            return;
        }
    };

    info!("Connected to Deepgram API");

    let (mut client_sender, mut client_receiver) = client_ws.split();
    let (mut deepgram_sender, mut deepgram_receiver) = deepgram_ws.split();

    // Multiplex audio frames and KeepAlive into a single Deepgram writer so
    // the `SplitSink` (which is not `Clone`) is only owned by one task.
    let (to_deepgram_tx, mut to_deepgram_rx) = mpsc::channel::<TungsteniteMessage>(64);

    // Writer: drain the channel and forward to Deepgram.
    let writer = tokio::spawn(async move {
        while let Some(msg) = to_deepgram_rx.recv().await {
            if deepgram_sender.send(msg).await.is_err() {
                break;
            }
        }
        let _ = deepgram_sender.close().await;
    });

    // KeepAlive: send {"type":"KeepAlive"} on a timer to prevent Deepgram's
    // 10-second NET-0001 timeout during silence (Deepgram docs — Audio Keep
    // Alive). The message must be a text frame, not binary.
    let keepalive_tx = to_deepgram_tx.clone();
    let keepalive = tokio::spawn(async move {
        let mut interval = tokio::time::interval(keepalive_interval());
        interval.tick().await; // skip the immediate first tick
        loop {
            interval.tick().await;
            if keepalive_tx
                .send(TungsteniteMessage::Text(KEEPALIVE_MSG.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Client → Deepgram: relay binary PCM audio. On client disconnect (close,
    // error, or stream end) send CloseStream so Deepgram flushes remaining
    // results gracefully.
    let client_tx = to_deepgram_tx.clone();
    let client_to_deepgram = tokio::spawn(async move {
        while let Some(msg) = client_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    if client_tx
                        .send(TungsteniteMessage::Binary(data.to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!("Client WebSocket error: {e}");
                    break;
                }
            }
        }
        // Graceful teardown — best-effort, the channel may already be closed.
        let _ = client_tx
            .send(TungsteniteMessage::Text(CLOSE_STREAM_MSG.into()))
            .await;
    });

    // Deepgram → Client: relay transcript JSON text frames.
    let deepgram_to_client = tokio::spawn(async move {
        while let Some(msg) = deepgram_receiver.next().await {
            match msg {
                Ok(TungsteniteMessage::Text(text)) => {
                    if client_sender
                        .send(Message::Text(text.into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(TungsteniteMessage::Close(_)) => break,
                Ok(_) => {}
                Err(e) => {
                    warn!("Deepgram WebSocket error: {e}");
                    break;
                }
            }
        }
    });

    // Wait for either direction to finish, then tear everything down.
    tokio::select! {
        _ = client_to_deepgram => {}
        _ = deepgram_to_client => {}
    }

    drop(to_deepgram_tx);
    keepalive.abort();
    writer.abort();

    info!("Deepgram proxy connection closed");
}
