//! Integration tests for issue #7: the concept merge-suggestion queue
//! (GET /merge-suggestions, POST approve/reject — ADR-0001/0002/0010).
//!
//! Concepts are created via the real submit→extract→accrete path; the merge
//! suggestion itself is seeded directly (its detection is covered by the graph
//! tests). Auth is bypassed by minting a session row directly (as in
//! `extraction.rs`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::{now_seconds, Db};
use second_brain_backend::error::Result;
use second_brain_backend::extractor::{ExtractedConcept, ExtractionResult};
use second_brain_backend::graph;
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// An LLM whose `extract` returns a canned result regardless of input, so the
/// accretion pipeline runs on deterministic concepts. The non-extraction
/// methods are stubs (these tests drive merge suggestions, not chat/refactor).
#[derive(Clone)]
struct ScriptedLlm {
    result: ExtractionResult,
}

#[async_trait]
impl Llm for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
        Ok("ScriptedLlm::synthesize (unused by merge-suggestion tests)".to_string())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
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

/// A scripted LLM whose `extract` returns a different result per call, so a
/// two-braindump cycle can create two distinct concepts deterministically.
struct SequencedLlm {
    calls: Mutex<usize>,
    results: Vec<ExtractionResult>,
}

#[async_trait]
impl Llm for SequencedLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
        Ok("SequencedLlm::synthesize (unused by merge-suggestion tests)".to_string())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        let mut calls = self.calls.lock().unwrap();
        let idx = *calls;
        *calls += 1;
        Ok(self.results.get(idx).cloned().unwrap_or_default())
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

fn concepts(labels: &[&str]) -> Vec<ExtractedConcept> {
    labels
        .iter()
        .map(|l| ExtractedConcept {
            label: l.to_string(),
        })
        .collect()
}

async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

async fn submit(app: &axum::Router, cookie: &http::HeaderValue, verbatim: &str) -> i64 {
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
    value["id"].as_i64().expect("id present")
}

fn app_with_llm(db: Db, llm: Arc<dyn Llm>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

async fn do_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<http::HeaderValue>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = &cookie {
        builder = builder.header(COOKIE, c);
    }
    let request = builder.body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Insert a pending concept merge suggestion directly (its detection is covered
/// by the graph ingest tests; the queue + approve/reject are the unit here).
async fn seed_suggestion(
    db: &Db,
    braindump_id: i64,
    new_concept_id: i64,
    existing_concept_id: i64,
) -> i64 {
    db.with_conn_test(move |conn| {
        let created_at = now_seconds();
        conn.execute(
            "INSERT INTO merge_suggestions
                (kind, braindump_id, new_concept_label, new_concept_id,
                 existing_concept_id, similarity, status, created_at)
             VALUES ('concept', ?1, 'label', ?2, ?3, 0.9, 'pending', ?4)",
            rusqlite::params![
                braindump_id,
                new_concept_id,
                existing_concept_id,
                created_at
            ],
        )?;
        Ok(conn.last_insert_rowid())
    })
    .await
    .unwrap()
}

/// Build an app with two distinct concepts (Maria, Beta), each extracted by its
/// own braindump, plus a pending suggestion linking Beta→Maria. Returns
/// everything the tests need to exercise the queue and verify graph state.
async fn seed_queue() -> (axum::Router, Db, http::HeaderValue, i64, i64, i64, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(SequencedLlm {
        calls: Mutex::new(0),
        results: vec![
            ExtractionResult {
                concepts: concepts(&["Maria"]),
                edges: vec![],
            },
            ExtractionResult {
                concepts: concepts(&["Beta"]),
                edges: vec![],
            },
        ],
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;
    let bd1 = submit(&app, &cookie, "maria").await;
    let bd2 = submit(&app, &cookie, "beta").await;
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let beta = graph::concept_id_for_label(&db, "Beta")
        .await
        .unwrap()
        .unwrap();
    let suggestion = seed_suggestion(&db, bd2, beta, maria).await;
    (app, db, cookie, bd1, bd2, maria, beta, suggestion)
}

#[tokio::test]
async fn get_merge_suggestions_lists_pending_pairs() {
    let (app, _db, cookie, _bd1, bd2, maria, beta, suggestion) = seed_queue().await;

    let (status, body) = do_request(&app, "GET", "/merge-suggestions", Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "list: {body}");
    let suggestions = body.as_array().expect("list is an array");
    assert_eq!(suggestions.len(), 1, "one pending suggestion: {body}");
    let s = &suggestions[0];
    assert_eq!(s["id"].as_i64().unwrap(), suggestion);
    assert_eq!(s["kind"].as_str().unwrap(), "concept");
    assert_eq!(s["status"].as_str().unwrap(), "pending");
    assert_eq!(s["braindump_id"].as_i64().unwrap(), bd2);
    assert_eq!(s["new_concept_id"].as_i64().unwrap(), beta);
    assert_eq!(s["existing_concept_id"].as_i64().unwrap(), maria);
}

#[tokio::test]
async fn approve_merges_concepts_unions_provenance_and_drops_suggestion() {
    let (app, db, cookie, bd1, bd2, maria, beta, suggestion) = seed_queue().await;

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/merge-suggestions/{suggestion}/approve"),
        Some(cookie.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "approve: {body}");

    // Union extraction provenance onto the surviving concept (ADR-0010).
    let mut prov = graph::concept_provenance(&db, maria).await.unwrap();
    prov.sort_unstable();
    assert_eq!(prov, vec![bd1, bd2]);
    // The fold concept is gone.
    assert!(
        graph::get_concept(&db, beta).await.unwrap().is_none(),
        "fold concept deleted on approve"
    );
    // The queue no longer lists the actioned suggestion.
    let (status, body) = do_request(&app, "GET", "/merge-suggestions", Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "list after approve: {body}");
    assert!(body.as_array().unwrap().is_empty(), "queue drained: {body}");
}

#[tokio::test]
async fn reject_keeps_concepts_separate_and_drops_suggestion() {
    let (app, db, cookie, bd1, _bd2, maria, beta, suggestion) = seed_queue().await;

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/merge-suggestions/{suggestion}/reject"),
        Some(cookie.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "reject: {body}");

    // Both concepts survive; provenance unchanged.
    assert!(
        graph::get_concept(&db, maria).await.unwrap().is_some(),
        "keeper survives reject"
    );
    assert!(
        graph::get_concept(&db, beta).await.unwrap().is_some(),
        "fold concept survives reject"
    );
    assert_eq!(
        graph::concept_provenance(&db, maria).await.unwrap(),
        vec![bd1],
        "provenance unchanged on reject"
    );
    // The suggestion is gone from the queue.
    let (status, body) = do_request(&app, "GET", "/merge-suggestions", Some(cookie)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.as_array().unwrap().is_empty(), "queue drained: {body}");
}

#[tokio::test]
async fn approve_missing_suggestion_is_404() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );
    let cookie = session_cookie(&db).await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/merge-suggestions/9999/approve",
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "approve missing: {body}");
}

#[tokio::test]
async fn reject_missing_suggestion_is_404() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );
    let cookie = session_cookie(&db).await;

    let (status, body) =
        do_request(&app, "POST", "/merge-suggestions/9999/reject", Some(cookie)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "reject missing: {body}");
}

#[tokio::test]
async fn merge_suggestion_routes_require_a_session() {
    let (app, _db, _cookie, _bd1, _bd2, _maria, _beta, _suggestion) = seed_queue().await;

    let (status, _body) = do_request(&app, "GET", "/merge-suggestions", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "list without session");

    let (status, _body) = do_request(&app, "POST", "/merge-suggestions/1/approve", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "approve without session");

    let (status, _body) = do_request(&app, "POST", "/merge-suggestions/1/reject", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "reject without session");
}
