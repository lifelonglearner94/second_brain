//! Integration tests for issue #84: async fire-and-forget braindump ingest
//! with background processing and the startup recovery scan.
//!
//! The existing ingest tests (`braindump.rs`, `extraction.rs`) exercise the
//! inline test runner (deterministic, no `tokio::spawn`); these tests exercise
//! the *spawned* path — `IngestRunner::new()` fires-and-forgets via
//! `tokio::spawn`, and `await_pending_ingests` drains the `JoinHandle`s so the
//! test can assert the post-background state deterministically.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::braindump::{
    await_pending_ingests, process_ingest_once, recover_pending, IngestRunner,
};
use second_brain_backend::db::{get_ingest_state, Db, BOOTSTRAP_ADMIN_USER_ID};
use second_brain_backend::error::Result;
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::graph;
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// Mint a session and return the `Cookie:` header value.
async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

async fn submit(app: &axum::Router, cookie: &http::HeaderValue, verbatim: &str) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/braindumps")
        .header(COOKIE, cookie)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "verbatim": verbatim }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "submit: {value}");
    value
}

async fn read_cleaned(app: &axum::Router, cookie: &http::HeaderValue, id: i64) -> Value {
    let request = Request::builder()
        .method("GET")
        .uri(format!("/braindumps/{id}"))
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Build app state with the *spawned* ingest runner (production shape) so the
/// route fire-and-forgets and the test drains via `await_pending_ingests`.
/// Returns (app, state) — state is held so the test can await its runner.
fn app_with_spawn_runner(db: Db, llm: Arc<dyn Llm>) -> (axum::Router, AppState) {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    state.ingest_runner = IngestRunner::new();
    let app = routes::router(state.clone());
    (app, state)
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

/// A scripted LLM whose `extract` returns a canned result (clean trims).
#[derive(Clone)]
struct ScriptedLlm {
    result: ExtractionResult,
}

#[async_trait]
impl Llm for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _: &str, _: &str) -> Result<String> {
        Ok("ScriptedLlm::synthesize (unused)".to_string())
    }
    async fn extract(&self, _: &str, _: &[String]) -> Result<ExtractionResult> {
        Ok(self.result.clone())
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

#[tokio::test]
async fn submit_returns_immediately_with_placeholder_then_background_completes() {
    let db = Db::open_in_memory().unwrap();
    let cookie = session_cookie(&db).await;
    let llm = Arc::new(ScriptedLlm {
        result: maria_endangers_q3(),
    });
    let (app, state) = app_with_spawn_runner(db.clone(), llm);

    let body = submit(&app, &cookie, "  maria endangers the q3 launch  ").await;
    let id = body["id"].as_i64().unwrap();
    // The response returns right away with a placeholder cleaned rendering —
    // no LLM call on the request path (issue #84).
    assert_eq!(
        body["cleaned"].as_str().unwrap(),
        "",
        "submit response cleaned is a placeholder"
    );
    assert!(body["created_at"].as_i64().unwrap() > 0);
    // The braindump is pending-processing immediately after submit.
    let state_row = get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state_row.status, "pending");

    // The background task runs out-of-band; drain it deterministically.
    await_pending_ingests(&state).await;

    // Now the cleaned rendering + concepts + edges landed.
    let after = read_cleaned(&app, &cookie, id).await;
    assert_eq!(
        after["cleaned"].as_str().unwrap(),
        "maria endangers the q3 launch"
    );
    let done = get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "complete", "{done:?}");
    assert_eq!(done.attempts, 1, "one attempt succeeded");
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .is_some(),
        "background accretion created the concept"
    );
    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        graph::edge_provenance(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
                .await
                .unwrap()
                .unwrap()
                .id
        )
        .await
        .unwrap(),
        vec![id],
        "edge accreted with the braindump as sole asserter"
    );
}

#[tokio::test]
async fn startup_recovery_scan_resumes_pending_braindump() {
    // Simulate a restart mid-processing: a braindump persisted as `pending`
    // (the previous backend crashed before the background task committed) and
    // no spawn was ever fired. The recovery scan picks it up and completes it.
    let db = Db::open_in_memory().unwrap();
    let llm: Arc<dyn Llm> = Arc::new(ScriptedLlm {
        result: maria_endangers_q3(),
    });
    // Persist verbatim directly (no route, no spawn) — the row is pending.
    let bd = second_brain_backend::braindump::submit_braindump(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        "  maria endangers the q3 launch  ",
    )
    .await
    .unwrap();
    assert_eq!(
        get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "pending"
    );
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .is_none()
    );

    // Recovery scan: one pending braindump resumed.
    let runner = IngestRunner::new();
    let resumed = recover_pending(&db, &llm, &runner).await.unwrap();
    assert_eq!(resumed, 1, "exactly one pending braindump resumed");
    runner.await_all().await;

    // The resumed pipeline completed the full clean → extract → accrete.
    let done = get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.status, "complete");
    let fetched =
        second_brain_backend::braindump::get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(fetched.cleaned, "maria endangers the q3 launch");
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .is_some(),
        "recovery scan accreted the graph"
    );
}

#[tokio::test]
async fn recovery_scan_skips_already_complete_braindumps() {
    // A braindump the previous run finished is `complete`; the recovery scan
    // must not re-process it (process_ingest_once is idempotent, and the scan
    // only selects `pending` rows).
    let db = Db::open_in_memory().unwrap();
    let llm: Arc<dyn Llm> = Arc::new(ScriptedLlm {
        result: ExtractionResult::default(),
    });
    let bd = second_brain_backend::braindump::submit_braindump(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        "already done",
    )
    .await
    .unwrap();
    // Run the pipeline once to completion.
    process_ingest_once(&db, BOOTSTRAP_ADMIN_USER_ID, llm.as_ref(), bd.id)
        .await
        .unwrap();
    second_brain_backend::db::set_ingest_status(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id, "complete", 1)
        .await
        .unwrap();

    let runner = IngestRunner::new();
    let resumed = recover_pending(&db, &llm, &runner).await.unwrap();
    assert_eq!(resumed, 0, "a complete braindump is not resumed");
    runner.await_all().await;
    assert_eq!(
        get_ingest_state(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        "complete"
    );
}

#[tokio::test]
async fn process_ingest_once_is_idempotent_over_completed_braindump() {
    let db = Db::open_in_memory().unwrap();
    let llm = ScriptedLlm {
        result: maria_endangers_q3(),
    };
    let bd = second_brain_backend::braindump::submit_braindump(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        "maria endangers q3 launch",
    )
    .await
    .unwrap();
    let first = process_ingest_once(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, bd.id)
        .await
        .unwrap();
    assert_eq!(first.concepts_created, 2);
    second_brain_backend::db::set_ingest_status(&db, BOOTSTRAP_ADMIN_USER_ID, bd.id, "complete", 1)
        .await
        .unwrap();

    // A second run is a no-op: the braindump is already complete.
    let second = process_ingest_once(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, bd.id)
        .await
        .unwrap();
    assert_eq!(
        second.concepts_created, 0,
        "idempotent re-run does not re-accrete"
    );
    assert_eq!(
        graph::concept_provenance(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
                .await
                .unwrap()
                .unwrap()
        )
        .await
        .unwrap(),
        vec![bd.id],
        "provenance unchanged after idempotent re-run"
    );
}
