//! Integration tests for issue #5: braindump ingest skeleton
//! (submit → clean → persist → read → error-correct, ADR-0007).
//!
//! Auth is bypassed by minting a session row directly — these tests exercise
//! the braindump write path, not WebAuthn (covered by `tests/auth.rs`).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::error::Result;
use second_brain_backend::extractor::ExtractionResult;
use second_brain_backend::llm::Llm;
use second_brain_backend::{db::Db, routes, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const VERBATIM: &str = "  the q3 review went off the rails  ";
const CLEANED: &str = "the q3 review went off the rails";

/// Mint a session and return the `Cookie:` header value to send with requests.
async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

/// Run a request against the app and collect (status, body-as-Value).
async fn do_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    cookie: Option<http::HeaderValue>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = &cookie {
        builder = builder.header(COOKIE, c);
    }
    let request = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(b.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// POST a braindump and return its id.
async fn submit(app: &axum::Router, cookie: &http::HeaderValue, verbatim: &str) -> Value {
    let (status, body) = do_request(
        app,
        "POST",
        "/braindumps",
        Some(json!({ "verbatim": verbatim })),
        Some(cookie.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "submit: {body}");
    body
}

/// An LLM whose `extract` counts how many times it was called, so tests can
/// assert the seam is wired on submit and on edit. The non-extraction methods
/// are stubs (these tests exercise the braindump write path, not chat/refactor).
#[derive(Default)]
struct CountingLlm {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Llm for CountingLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
        Ok("CountingLlm::synthesize (unused by braindump tests)".to_string())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ExtractionResult::default())
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

/// Build app state with a recording LLM; return (app, calls-counter).
fn app_with_recording_llm(db: Db) -> (axum::Router, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let llm = Arc::new(CountingLlm {
        calls: calls.clone(),
    });
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    (routes::router(state), calls)
}

#[tokio::test]
async fn submit_persists_verbatim_immediately_with_placeholder_cleaned() {
    // Issue #84: submit is fire-and-forget. The verbatim is persisted
    // immediately and the response returns right away with an empty
    // (placeholder) cleaned rendering — the LLM cleaning runs in the
    // background. The response still carries id + created_at so the UI can
    // confirm the submit landed; the cleaned rendering lands once the
    // background task commits (here it runs inline via the test runner, so a
    // subsequent GET sees it).
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app = routes::router(AppState::for_tests(db));

    let body = submit(&app, &cookie, VERBATIM).await;
    let id = body["id"].as_i64().expect("id present");
    assert!(id > 0, "id is a positive surrogate: {body}");
    assert_eq!(body["verbatim"].as_str().unwrap(), VERBATIM);
    assert_eq!(
        body["cleaned"].as_str().unwrap(),
        "",
        "submit response carries a placeholder cleaned rendering (background task not on the request path)"
    );
    assert!(body["created_at"].as_i64().unwrap() > 0, "timestamp set");

    // The background ingest ran (inline in tests); a GET now returns the
    // LLM-cleaned rendering populated out-of-band.
    let (status, body) = do_request(
        &app,
        "GET",
        &format!("/braindumps/{id}"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read: {body}");
    assert_eq!(body["verbatim"].as_str().unwrap(), VERBATIM);
    assert_eq!(body["cleaned"].as_str().unwrap(), CLEANED);
}

#[tokio::test]
async fn read_returns_both_verbatim_and_cleaned() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app = routes::router(AppState::for_tests(db));

    let submitted = submit(&app, &cookie, VERBATIM).await;
    let id = submitted["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "GET",
        &format!("/braindumps/{id}"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read: {body}");
    assert_eq!(body["id"].as_i64().unwrap(), id);
    assert_eq!(body["verbatim"].as_str().unwrap(), VERBATIM);
    assert_eq!(body["cleaned"].as_str().unwrap(), CLEANED);
    assert_eq!(
        body["created_at"].as_i64().unwrap(),
        submitted["created_at"].as_i64().unwrap(),
        "read returns the original submit timestamp"
    );
}

#[tokio::test]
async fn read_missing_braindump_is_404() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app = routes::router(AppState::for_tests(db));

    let (status, _body) = do_request(&app, "GET", "/braindumps/9999", None, Some(cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn edit_overwrites_verbatim_in_place_recleans_and_reruns_extractor() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let (app, calls) = app_with_recording_llm(db);

    let submitted = submit(&app, &cookie, VERBATIM).await;
    let id = submitted["id"].as_i64().unwrap();
    let created_at = submitted["created_at"].as_i64().unwrap();
    let calls_after_submit = calls.load(Ordering::SeqCst);
    assert_eq!(
        calls_after_submit, 1,
        "extractor runs once on submit (stub returns no concepts/edges)"
    );

    let new_verbatim = "  the q4 review went much better  ";
    let new_cleaned = "the q4 review went much better";
    let (status, body) = do_request(
        &app,
        "PATCH",
        &format!("/braindumps/{id}"),
        Some(json!({ "verbatim": new_verbatim })),
        Some(cookie.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "edit: {body}");
    assert_eq!(body["id"].as_i64().unwrap(), id, "id stable across edit");
    assert_eq!(
        body["created_at"].as_i64().unwrap(),
        created_at,
        "timestamp stable across edit (ADR-0007: overwrite in place)"
    );
    assert_eq!(body["verbatim"].as_str().unwrap(), new_verbatim);
    assert_eq!(
        body["cleaned"].as_str().unwrap(),
        new_cleaned,
        "edit re-runs the cleaner on the corrected verbatim"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        calls_after_submit + 1,
        "edit re-runs the (stubbed) extractor"
    );

    let (status, body) = do_request(
        &app,
        "GET",
        &format!("/braindumps/{id}"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["verbatim"].as_str().unwrap(), new_verbatim);
    assert_eq!(body["cleaned"].as_str().unwrap(), new_cleaned);
}

#[tokio::test]
async fn edit_on_missing_braindump_is_404() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app = routes::router(AppState::for_tests(db));

    let (status, _body) = do_request(
        &app,
        "PATCH",
        "/braindumps/9999",
        Some(json!({ "verbatim": "x" })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn verbatim_is_immutable_except_via_edit() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let app = routes::router(AppState::for_tests(db));

    let submitted = submit(&app, &cookie, VERBATIM).await;
    let id = submitted["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "GET",
        &format!("/braindumps/{id}"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["verbatim"].as_str().unwrap(),
        VERBATIM,
        "verbatim unchanged on read"
    );
}

#[tokio::test]
async fn submit_runs_the_stub_extractor_seam() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let (app, calls) = app_with_recording_llm(db);

    submit(&app, &cookie, VERBATIM).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "extractor seam wired on submit"
    );
}

#[tokio::test]
async fn braindump_routes_require_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let (status, _body) = do_request(
        &app,
        "POST",
        "/braindumps",
        Some(json!({ "verbatim": "x" })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "submit without session");

    let (status, _body) = do_request(&app, "GET", "/braindumps/1", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "read without session");
}
