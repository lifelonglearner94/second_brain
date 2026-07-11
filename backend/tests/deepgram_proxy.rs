//! Integration tests for the Deepgram WebSocket proxy (issue #90).
//!
//! The proxy relays binary PCM audio from the client to Deepgram and text JSON
//! transcripts back, keeping the API key server-side (ADR-0004). These tests
//! cover three layers:
//!
//! 1. **HTTP-level guards** — the route is behind `require_session` (401 without
//!    a session) and returns 503 when `DEEPGRAM_API_KEY` is unset.
//! 2. **End-to-end relay** — a mock WebSocket server stands in for Deepgram; a
//!    real WebSocket client connects through the proxy, sends audio, and
//!    receives a canned transcript.
//! 3. **Protocol correctness** — KeepAlive is sent during silence (prevents
//!    Deepgram's 10-second NET-0001 timeout), and CloseStream is sent on
//!    client disconnect (graceful teardown).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::header::COOKIE;
use http::{Request, StatusCode};
use serial_test::serial;
use tokio::net::TcpListener;
use tokio_tungstenite::{
    accept_async, connect_async, tungstenite::handshake::client::generate_key,
    tungstenite::protocol::Message as TungsteniteMessage,
};
use tower::ServiceExt;

use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::{db::Db, routes, state::AppState};

/// Mint a session for the bootstrap admin and return the Cookie header value.
async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

/// A canned Deepgram transcript message — the same JSON shape the frontend
/// parses (`is_final` + `channel.alternatives[0].transcript`).
const CANNED_TRANSCRIPT: &str = r#"{"type":"Results","is_final":true,"channel":{"alternatives":[{"transcript":"hello world","confidence":1.0,"words":[]}]}}"#;

// ---------------------------------------------------------------------------
// 1. HTTP-level guards
// ---------------------------------------------------------------------------

/// The route is behind `require_session` — a request without a session cookie
/// gets 401, regardless of whether the API key is set.
#[tokio::test]
#[serial(deepgram)]
async fn deepgram_proxy_requires_session() {
    std::env::set_var("DEEPGRAM_API_KEY", "test-key");

    let db = Db::open_in_memory().expect("in-memory db");
    let app = routes::router(AppState::for_tests(db));

    let request = ws_upgrade_request_body("/stt/deepgram", None);
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    std::env::remove_var("DEEPGRAM_API_KEY");
}

/// With a valid session but no `DEEPGRAM_API_KEY`, the handler returns 503
/// (graceful dev/CI degradation, mirroring how `GEMINI_API_KEY` unset falls
/// back to the fake LLM). This needs a real server because the
/// `WebSocketUpgrade` extractor returns 426 under `oneshot` (no real TCP
/// connection to upgrade).
#[tokio::test]
#[serial(deepgram)]
async fn deepgram_proxy_refuses_without_api_key() {
    std::env::remove_var("DEEPGRAM_API_KEY");

    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app_addr = start_app(db).await;

    // The proxy should return 503 instead of 101 Switching Protocols, so the
    // WebSocket handshake fails.
    let result = connect_ws_client(app_addr, &cookie).await;
    assert!(
        result.is_err(),
        "connection should fail with 503, not upgrade to WebSocket"
    );

    std::env::remove_var("DEEPGRAM_API_KEY");
    std::env::remove_var("DEEPGRAM_WS_URL");
    std::env::remove_var("DEEPGRAM_KEEPALIVE_SECS");
}

// ---------------------------------------------------------------------------
// 2. End-to-end relay with a mock Deepgram server
// ---------------------------------------------------------------------------

/// A message recorded by the mock Deepgram server.
#[derive(Debug, PartialEq, Eq)]
enum MockMsg {
    Binary(usize),
    Text(String),
}

/// Start a mock Deepgram WebSocket server on a random port.
///
/// The mock:
/// - Accepts one connection (the proxy's outbound connection).
/// - Records all received messages (binary audio, KeepAlive, CloseStream).
/// - Sends a canned transcript immediately after receiving the first binary
///   frame, so the relay test can verify text flows back to the client.
///
/// Returns the `ws://` URL and a channel receiver for the recorded messages.
fn spawn_mock_deepgram() -> (String, tokio::sync::oneshot::Receiver<Vec<MockMsg>>) {
    let (tx, rx) = tokio::sync::oneshot::channel::<Vec<MockMsg>>();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let url = format!("ws://{addr}/v1/listen");

    // Set the mock server's TCP listener to non-blocking so tokio can accept.
    listener.set_nonblocking(true).unwrap();

    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_async(stream).await.unwrap();

        let mut received = Vec::new();
        let mut sent_transcript = false;

        while let Some(msg) = ws.next().await {
            match msg {
                Ok(TungsteniteMessage::Binary(data)) => {
                    received.push(MockMsg::Binary(data.len()));
                    // Send the canned transcript once binary audio arrives.
                    if !sent_transcript {
                        ws.send(TungsteniteMessage::Text(CANNED_TRANSCRIPT.into()))
                            .await
                            .unwrap();
                        sent_transcript = true;
                    }
                }
                Ok(TungsteniteMessage::Text(text)) => {
                    received.push(MockMsg::Text(text.to_string()));
                    // CloseStream = client disconnected — stop recording.
                    if text.contains("CloseStream") {
                        let _ = ws.close(None).await;
                        break;
                    }
                }
                Ok(TungsteniteMessage::Close(_)) => break,
                Err(_) => break,
                _ => {}
            }
        }

        let _ = tx.send(received);
    });

    (url, rx)
}

/// Build a WebSocket upgrade HTTP request with an empty body (for `oneshot`).
fn ws_upgrade_request_body(
    uri: &str,
    cookie: Option<&http::HeaderValue>,
) -> Request<axum::body::Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key());
    if let Some(c) = cookie {
        builder = builder.header(COOKIE, c);
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

/// Start the axum app on a random port and return the address it's listening on.
async fn start_app(db: Db) -> std::net::SocketAddr {
    let app = routes::router(AppState::for_tests(db));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give the server a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// Connect a WebSocket client to the proxy with a session cookie.
/// Returns `Err` if the server does not upgrade (e.g. 503 when the API key
/// is unset).
async fn connect_ws_client(
    app_addr: std::net::SocketAddr,
    cookie: &http::HeaderValue,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    let url = format!("ws://{app_addr}/stt/deepgram");
    let request = Request::builder()
        .uri(&url)
        .header("host", app_addr.to_string())
        .header("upgrade", "websocket")
        .header("connection", "upgrade")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", generate_key())
        .header(COOKIE, cookie)
        .body(())
        .unwrap();
    let (ws, _) = connect_async(request).await?;
    Ok(ws)
}

/// Binary PCM audio is relayed client → Deepgram, and transcript JSON is
/// relayed Deepgram → client.
#[tokio::test]
#[serial(deepgram)]
async fn deepgram_proxy_relays_audio_and_transcripts() {
    let (mock_url, mock_rx) = spawn_mock_deepgram();

    std::env::set_var("DEEPGRAM_API_KEY", "test-key");
    std::env::set_var("DEEPGRAM_WS_URL", &mock_url);
    std::env::set_var("DEEPGRAM_KEEPALIVE_SECS", "3600"); // disable keepalive for this test

    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app_addr = start_app(db).await;

    let mut client = connect_ws_client(app_addr, &cookie)
        .await
        .expect("WebSocket upgrade should succeed with API key set");

    // Send a short Int16 PCM buffer (256 samples of silence, little-endian).
    let audio: Vec<u8> = (0..256i16).flat_map(|s| s.to_le_bytes()).collect();
    client
        .send(TungsteniteMessage::Binary(audio))
        .await
        .unwrap();

    // The mock server sends a canned transcript after receiving audio.
    // Wait for it to arrive at the client through the relay.
    let transcript = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(TungsteniteMessage::Text(t))) => return Some(t),
                Some(Ok(_)) => continue,
                _ => return None,
            }
        }
    })
    .await
    .expect("timed out waiting for transcript")
    .expect("stream ended without a transcript");

    assert!(transcript.contains("hello world"));
    assert!(transcript.contains("\"is_final\":true"));

    // Close the client — the proxy should send CloseStream to the mock.
    client.close(None).await.ok();

    // Wait for the mock server to report what it received.
    let received = tokio::time::timeout(Duration::from_secs(5), mock_rx)
        .await
        .expect("mock server did not report")
        .expect("mock server channel dropped");

    // The mock should have received at least one binary frame (the audio).
    assert!(
        received
            .iter()
            .any(|m| matches!(m, MockMsg::Binary(n) if *n > 0)),
        "mock Deepgram should have received binary audio, got: {received:?}"
    );
    // The mock should have received a CloseStream text message.
    assert!(
        received
            .iter()
            .any(|m| matches!(m, MockMsg::Text(t) if t.contains("CloseStream"))),
        "mock Deepgram should have received CloseStream, got: {received:?}"
    );

    std::env::remove_var("DEEPGRAM_API_KEY");
    std::env::remove_var("DEEPGRAM_WS_URL");
    std::env::remove_var("DEEPGRAM_KEEPALIVE_SECS");
}

/// KeepAlive messages are sent to Deepgram on a timer to prevent the 10-second
/// NET-0001 timeout during silence (Deepgram docs — Audio Keep Alive).
#[tokio::test]
#[serial(deepgram)]
async fn deepgram_proxy_sends_keepalive_during_silence() {
    let (mock_url, mock_rx) = spawn_mock_deepgram();

    std::env::set_var("DEEPGRAM_API_KEY", "test-key");
    std::env::set_var("DEEPGRAM_WS_URL", &mock_url);
    std::env::set_var("DEEPGRAM_KEEPALIVE_SECS", "1"); // 1-second interval for a fast test

    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app_addr = start_app(db).await;

    let mut client = connect_ws_client(app_addr, &cookie)
        .await
        .expect("WebSocket upgrade should succeed");

    // Don't send any audio — just wait for the KeepAlive to fire.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    client.close(None).await.ok();

    let received = tokio::time::timeout(Duration::from_secs(5), mock_rx)
        .await
        .expect("mock server did not report")
        .expect("mock server channel dropped");

    // At least one KeepAlive should have arrived in ~1.5 seconds.
    let keepalive_count = received
        .iter()
        .filter(|m| matches!(m, MockMsg::Text(t) if t.contains("KeepAlive")))
        .count();
    assert!(
        keepalive_count >= 1,
        "expected at least one KeepAlive, got: {received:?}"
    );

    std::env::remove_var("DEEPGRAM_API_KEY");
    std::env::remove_var("DEEPGRAM_WS_URL");
    std::env::remove_var("DEEPGRAM_KEEPALIVE_SECS");
}
