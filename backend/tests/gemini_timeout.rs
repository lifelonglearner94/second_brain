//! Issue #100: the Gemini HTTP client must apply a request timeout so a hung
//! connection (server accepts the TCP handshake but never sends an HTTP
//! response) becomes `Error::TransientLlm` instead of parking the ingest
//! background task forever. A real `GeminiClient` is pointed at a mock TCP
//! server that accepts then parks, with short env-configured timeouts so the
//! case is fast and hermetic.

use std::time::Duration;

use serial_test::serial;
use tokio::net::TcpListener;

use second_brain_backend::error::Error;
use second_brain_backend::gemini::GeminiClient;
use second_brain_backend::llm::Llm;

/// Start a mock server that accepts one TCP connection then never writes a
/// response - reproducing the "hung connection" from issue #100. The connection
/// is held open so reqwest's request timeout (not the connect timeout) is the
/// one that fires.
async fn spawn_parking_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (_stream, _peer) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    addr
}

/// A hung connection times out and surfaces as `Error::TransientLlm`, not an
/// infinite hang. The outer `tokio::time::timeout` bounds the call so a
/// regression (no reqwest timeout) fails the test instead of stalling the
/// suite.
#[tokio::test]
#[serial(gemini_env)]
async fn hung_connection_times_out_as_transient() {
    let addr = spawn_parking_server().await;

    std::env::set_var("GEMINI_API_KEY", "test-key");
    std::env::set_var("GEMINI_BASE", format!("http://{addr}"));
    // Generous connect timeout (localhost is instant); the 1s request timeout
    // is what fires against the parking server.
    std::env::set_var("GEMINI_CONNECT_TIMEOUT_SECS", "5");
    std::env::set_var("GEMINI_REQUEST_TIMEOUT_SECS", "1");

    let client = GeminiClient::from_env()
        .expect("from_env builds")
        .expect("GEMINI_API_KEY was set");

    let result = tokio::time::timeout(Duration::from_secs(5), client.clean("hi")).await;

    std::env::remove_var("GEMINI_API_KEY");
    std::env::remove_var("GEMINI_BASE");
    std::env::remove_var("GEMINI_CONNECT_TIMEOUT_SECS");
    std::env::remove_var("GEMINI_REQUEST_TIMEOUT_SECS");

    let err = result
        .expect("clean must return once the timeout fires, not hang")
        .expect_err("a hung connection must be an error");
    assert!(
        matches!(err, Error::TransientLlm(_)),
        "hung connection must map to TransientLlm, got: {err:?}"
    );
}
