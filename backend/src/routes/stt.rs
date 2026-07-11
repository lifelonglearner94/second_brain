use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as TungsteniteMessage};
use tracing::{error, info, warn};

use crate::{error::Error, state::AppState};

/// WebSocket proxy route for Deepgram STT.
/// Relays binary PCM audio from client to Deepgram and text JSON transcripts back.
pub async fn deepgram_proxy(
    ws: WebSocketUpgrade,
    State(_state): State<AppState>,
) -> Result<impl IntoResponse, Error> {
    // Check if DEEPGRAM_API_KEY is set
    let api_key = std::env::var("DEEPGRAM_API_KEY").map_err(|_| {
        warn!("DEEPGRAM_API_KEY not set, refusing WebSocket upgrade");
        Error::ServiceUnavailable("Deepgram API key not configured".to_string())
    })?;

    Ok(ws.on_upgrade(move |socket| handle_socket(socket, api_key)))
}

async fn handle_socket(mut client_ws: WebSocket, api_key: String) {
    info!("Client connected to Deepgram proxy");

    // Build Deepgram WebSocket URL with query parameters
    let deepgram_url = "wss://api.deepgram.com/v1/listen?model=nova-3&language=de&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true";

    // Create request with Authorization header
    let request = match tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(deepgram_url)
        .header("Authorization", format!("Token {}", api_key))
        .header("Host", "api.deepgram.com")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", tokio_tungstenite::tungstenite::handshake::client::generate_key())
        .body(())
    {
        Ok(req) => req,
        Err(e) => {
            error!("Failed to build Deepgram request: {}", e);
            let _ = client_ws.close().await;
            return;
        }
    };

    let (deepgram_ws, _) = match connect_async(request).await {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to connect to Deepgram: {}", e);
            // Try to send error to client before closing
            let _ = client_ws.close().await;
            return;
        }
    };

    info!("Connected to Deepgram API");

    let (mut client_sender, mut client_receiver) = client_ws.split();
    let (mut deepgram_sender, mut deepgram_receiver) = deepgram_ws.split();

    // Create channels for coordinated shutdown
    let (shutdown_tx, mut shutdown_rx) = mpsc::channel::<()>(1);
    let shutdown_tx_clone = shutdown_tx.clone();

    // Task 1: Client -> Deepgram (binary audio frames)
    let client_to_deepgram = tokio::spawn(async move {
        while let Some(msg) = client_receiver.next().await {
            match msg {
                Ok(Message::Binary(data)) => {
                    // Forward binary PCM audio to Deepgram
                    if let Err(e) = deepgram_sender
                        .send(TungsteniteMessage::Binary(data.to_vec()))
                        .await
                    {
                        error!("Failed to send to Deepgram: {}", e);
                        break;
                    }
                }
                Ok(Message::Close(_)) => {
                    info!("Client closed connection");
                    break;
                }
                Ok(_) => {
                    // Ignore text/ping/pong from client
                }
                Err(e) => {
                    error!("Client WebSocket error: {}", e);
                    break;
                }
            }
        }
        let _ = shutdown_tx.send(()).await;
    });

    // Task 2: Deepgram -> Client (text JSON transcripts)
    let deepgram_to_client = tokio::spawn(async move {
        while let Some(msg) = deepgram_receiver.next().await {
            match msg {
                Ok(TungsteniteMessage::Text(text)) => {
                    // Forward JSON transcript to client
                    if let Err(e) = client_sender.send(Message::Text(text.into())).await {
                        error!("Failed to send to client: {}", e);
                        break;
                    }
                }
                Ok(TungsteniteMessage::Close(_)) => {
                    info!("Deepgram closed connection");
                    break;
                }
                Ok(_) => {
                    // Ignore binary/ping/pong from Deepgram
                }
                Err(e) => {
                    error!("Deepgram WebSocket error: {}", e);
                    break;
                }
            }
        }
        let _ = shutdown_tx_clone.send(()).await;
    });

    // Wait for either task to complete (indicating disconnect)
    let _ = shutdown_rx.recv().await;

    // Abort both tasks to ensure clean shutdown
    client_to_deepgram.abort();
    deepgram_to_client.abort();

    info!("Deepgram proxy connection closed");
}
