//! Dedicated test binary for the issue #85 logging acceptance criterion.
//!
//! Lives in its own binary (separate from `async_ingest.rs`) so it can install
//! a *global* tracing subscriber (`set_global_default`) without racing other
//! tests in the same binary. Production wires `LogBufferLayer` globally via
//! `main::init_tracing`; the spawned background ingest tasks emit their retry
//! attempts (WARN per transient failure, INFO on success) through that global
//! subscriber, so the admin log buffer captures them on any thread. This
//! binary reproduces that wiring with a single test.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::braindump::{await_pending_ingests, IngestRunner};
use second_brain_backend::db::{get_ingest_state, Db, BOOTSTRAP_ADMIN_USER_ID};
use second_brain_backend::error::{Error, Result};
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::llm::Llm;
use second_brain_backend::logs::LogBufferLayer;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::EnvFilter;

/// An LLM whose `extract` fails transiently for the first `fail_n` calls then
/// succeeds (issue #85: Gemini 5xx / rate-limited / transport).
struct TransientThenSuccessLlm {
    fail_n: usize,
    extract_calls: Arc<AtomicUsize>,
    result: ExtractionResult,
}

#[async_trait]
impl Llm for TransientThenSuccessLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _: &str, _: &str) -> Result<String> {
        Ok("TransientThenSuccessLlm::synthesize (unused)".to_string())
    }
    async fn extract(&self, _: &str, _: &[String]) -> Result<ExtractionResult> {
        let n = self.extract_calls.fetch_add(1, Ordering::SeqCst);
        if n < self.fail_n {
            Err(Error::TransientLlm(format!(
                "gemini generate 503: overloaded (attempt {})",
                n + 1
            )))
        } else {
            Ok(self.result.clone())
        }
    }
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    fn dim(&self) -> usize {
        64
    }
}

fn maria_endangers_q3() -> ExtractionResult {
    ExtractionResult {
        concepts: vec![
            ExtractedConcept {
                label: "Maria".into(),
            },
            ExtractedConcept {
                label: "Q3 launch".into(),
            },
        ],
        edges: vec![ExtractedEdge {
            from_label: "Maria".into(),
            type_slug: "endangers".into(),
            to_label: "Q3 launch".into(),
        }],
    }
}

async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

/// Install a global tracing subscriber that feeds the admin log buffer — the
/// production wiring (`main::init_tracing` installs `LogBufferLayer`
/// globally). `set_global_default` is process-wide; this is the only test in
/// this binary, so it succeeds on the first call and the spawned background
/// task's events are captured on any thread.
static GLOBAL_INSTALLED: std::sync::Once = std::sync::Once::new();
fn install_global_log_subscriber(log_buffer: second_brain_backend::logs::LogBuffer) {
    GLOBAL_INSTALLED.call_once(|| {
        let subscriber = tracing_subscriber::registry()
            .with(LogBufferLayer::new(log_buffer))
            .with(EnvFilter::new("info,second_brain_backend=debug"));
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

#[tokio::test]
async fn retry_attempts_are_logged_to_the_admin_log_buffer() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let llm = Arc::new(TransientThenSuccessLlm {
        fail_n: 1,
        extract_calls: Arc::new(AtomicUsize::new(0)),
        result: maria_endangers_q3(),
    });
    // Spawned runner (production shape): the background task runs out-of-band
    // and `await_pending_ingests` drains it. The global subscriber captures
    // its events regardless of which worker thread they land on.
    let mut state = AppState::for_tests(db.clone());
    state.llm = llm;
    state.ingest_runner = IngestRunner::new();
    install_global_log_subscriber(state.log_buffer.clone());
    let app = routes::router(state.clone());

    let request = Request::builder()
        .method("POST")
        .uri("/braindumps")
        .header(COOKIE, &cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "verbatim": "maria endangers q3 launch" }).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let id = body["id"].as_i64().unwrap();
    await_pending_ingests(&state).await;

    let recent = state.log_buffer.recent(usize::MAX);
    // One WARN per transient attempt, carrying the braindump id + reason.
    let warns: Vec<_> = recent
        .iter()
        .filter(|e| e.level == "WARN" && e.message.contains("ingest attempt failed"))
        .collect();
    assert_eq!(warns.len(), 1, "exactly one failed-attempt log: {recent:?}");
    assert_eq!(warns[0].fields["braindump_id"], serde_json::json!(id));
    assert_eq!(warns[0].fields["transient"], serde_json::json!(true));
    assert!(
        warns[0].fields["error"]
            .as_str()
            .unwrap()
            .contains("overloaded"),
        "failure reason logged: {:?}",
        warns[0].fields["error"]
    );
    // One INFO on the successful completion, carrying the accretion outcome.
    let success: Vec<_> = recent
        .iter()
        .filter(|e| e.level == "INFO" && e.message.contains("accrete complete"))
        .collect();
    assert_eq!(success.len(), 1, "one completion log: {recent:?}");
    assert_eq!(success[0].fields["braindump_id"], serde_json::json!(id));
    assert_eq!(success[0].fields["created"], serde_json::json!(2));
    assert_eq!(success[0].fields["edges_created"], serde_json::json!(1));
    assert_eq!(
        get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "complete"
    );
}
