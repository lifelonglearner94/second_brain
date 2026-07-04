//! Integration tests for issue #12: the Thematic Read Model endpoint
//! `GET /thematic` (ADR-0008).
//!
//! The backend owns the Louvain partition; the endpoint returns the current
//! projection with ephemeral "Group N for this session" labels. Concepts/edges
//! are created via the real submit→extract→accrete path; auth is bypassed by
//! minting a session row directly (as in `delta_sync.rs`). The extractor is a
//! scripted stand-in so the accretion pipeline runs on deterministic
//! concepts/edges — no Gemini call.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::error::Result;
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

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
        Ok("ScriptedLlm::synthesize (unused by thematic tests)".to_string())
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

fn concepts(labels: &[&str]) -> Vec<ExtractedConcept> {
    labels
        .iter()
        .map(|l| ExtractedConcept {
            label: l.to_string(),
        })
        .collect()
}

fn edge(from: &str, type_slug: &str, to: &str) -> ExtractedEdge {
    ExtractedEdge {
        from_label: from.to_string(),
        type_slug: type_slug.to_string(),
        to_label: to.to_string(),
    }
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

async fn thematic(app: &axum::Router, cookie: &http::HeaderValue) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("GET")
        .uri("/thematic")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn app_with_llm(db: Db, llm: Arc<dyn Llm>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

#[tokio::test]
async fn get_thematic_returns_the_current_partition_with_session_labels() {
    // Two disjoint edges → two clusters. The endpoint returns the partition
    // with ADR-0008 ephemeral "Group N for this session" labels and the full
    // concept coverage, for the frontend to render.
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch", "Alpha", "Beta"]),
                edges: vec![
                    edge("Maria", "endangers", "Q3 launch"),
                    edge("Alpha", "helps", "Beta"),
                ],
            },
        }),
    );
    let cookie = session_cookie(&db).await;
    submit(&app, &cookie, "maria endangers q3 launch").await;
    submit(&app, &cookie, "alpha helps beta").await;

    let (status, body) = thematic(&app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "thematic: {body}");
    assert_eq!(body["concept_count"], 4, "all four concepts projected");
    let clusters = body["clusters"].as_array().expect("clusters array");
    assert_eq!(clusters.len(), 2, "two disjoint-edge clusters");
    let labels: Vec<&str> = clusters
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        vec!["Group 1 for this session", "Group 2 for this session"],
        "ADR-0008 ephemeral session labels, largest first"
    );
    // Every concept appears in exactly one cluster, with its label paired.
    let mut all_ids: Vec<i64> = clusters
        .iter()
        .flat_map(|c| {
            c["concept_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_i64().unwrap())
        })
        .collect();
    all_ids.sort_unstable();
    assert_eq!(all_ids.len(), 4, "every concept in exactly one cluster");
    let mut seen = all_ids.clone();
    seen.dedup();
    assert_eq!(seen.len(), 4, "no concept listed twice");
    for c in clusters {
        assert_eq!(
            c["concept_ids"].as_array().unwrap().len(),
            c["concept_labels"].as_array().unwrap().len(),
            "labels and ids paired"
        );
    }
}

#[tokio::test]
async fn get_thematic_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let request = Request::builder()
        .method("GET")
        .uri("/thematic")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_thematic_on_an_empty_graph_returns_an_empty_partition() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );
    let cookie = session_cookie(&db).await;

    let (status, body) = thematic(&app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "thematic: {body}");
    assert_eq!(body["concept_count"], 0);
    assert!(
        body["clusters"].as_array().unwrap().is_empty(),
        "no clusters on an empty graph"
    );
}
