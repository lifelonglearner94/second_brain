//! Integration tests for issue #8: the retrieval read path (ADR-0004).
//!
//! Seed-then-expand through the `POST /retrieve` route: the query is
//! FakeEmbedding-embedded, concept-embedding KNN seeds, typed-edge graph
//! traversal expands, braindumps from the subgraph (plus braindump-embedding
//! backfill) form the context. Unanchored queries fall back to
//! braindump-vector-direct. Auth is bypassed by minting a session row directly
//! (as in `tests/extraction.rs`); the extractor is a scripted stand-in so the
//! accretion pipeline runs on deterministic concepts/edges — no Gemini call.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::error::Result;
use second_brain_backend::extractor::{
    ExtractedConcept, ExtractedEdge, ExtractionResult, Extractor,
};
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// An extractor that returns a canned result regardless of input, so the
/// accretion pipeline runs on deterministic concepts/edges.
#[derive(Clone)]
struct ScriptedExtractor {
    result: ExtractionResult,
}

#[async_trait]
impl Extractor for ScriptedExtractor {
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        Ok(self.result.clone())
    }
}

/// A scripted extractor that returns a different result per call, so a single
/// submit cycle can drive distinct concept/edge sets deterministically.
struct SequencedExtractor {
    calls: Mutex<usize>,
    results: Vec<ExtractionResult>,
}

#[async_trait]
impl Extractor for SequencedExtractor {
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

async fn retrieve(
    app: &axum::Router,
    cookie: &http::HeaderValue,
    query: &str,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/retrieve")
        .header(COOKIE, cookie)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "query": query }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

fn app_with_extractor(db: Db, extractor: Arc<dyn Extractor>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.extractor = extractor;
    routes::router(state)
}

#[tokio::test]
async fn retrieve_finds_graph_linked_braindump_via_expansion() {
    // ADR-0004 canonical demo: a braindump graph-linked
    // `Maria —[endangers]→ Q3 launch` but not containing "Q3" is found by
    // querying "Q3" — seed on Q3 launch, traverse the incoming edge to Maria,
    // collect her braindump.
    let db = Db::open_in_memory().unwrap();
    let extractor = Arc::new(ScriptedExtractor {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
    });
    let app = app_with_extractor(db.clone(), extractor);
    let cookie = session_cookie(&db).await;

    let bd = submit(&app, &cookie, "maria leaving tanks the timeline").await;

    let (status, body) = retrieve(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "retrieve: {body}");
    assert_eq!(body["mode"], "seed_then_expand");
    let dumps = body["braindumps"].as_array().expect("braindumps array");
    let found = dumps
        .iter()
        .find(|b| b["id"].as_i64() == Some(bd))
        .expect("graph-linked braindump found via expansion");
    assert_eq!(found["source"], "subgraph");
    assert!(
        !found["verbatim"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("q3"),
        "the found braindump must not lexically contain the query word"
    );
    let paths = body["paths"].as_array().expect("paths array");
    assert!(
        paths.iter().any(|e| {
            e["source_concept_label"] == "Maria"
                && e["target_concept_label"] == "Q3 launch"
                && e["edge_type"] == "endangers"
        }),
        "traversed edge path present: {paths:?}"
    );
}

#[tokio::test]
async fn retrieve_backfills_strays_the_graph_missed() {
    // A braindump whose concept does not seed and is not graph-connected,
    // but whose text matches the query — found by braindump-embedding backfill.
    let db = Db::open_in_memory().unwrap();
    let extractor = Arc::new(SequencedExtractor {
        calls: Mutex::new(0),
        results: vec![
            ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch"]),
                edges: vec![edge("Maria", "endangers", "Q3 launch")],
            },
            ExtractionResult {
                concepts: concepts(&["Risk assessment"]),
                edges: vec![],
            },
        ],
    });
    let app = app_with_extractor(db.clone(), extractor);
    let cookie = session_cookie(&db).await;

    let _bd_graph = submit(&app, &cookie, "maria endangers the q3 launch").await;
    let bd_stray = submit(&app, &cookie, "q3 risk assessment notes").await;

    let (status, body) = retrieve(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "retrieve: {body}");
    let dumps = body["braindumps"].as_array().expect("braindumps array");
    let stray = dumps
        .iter()
        .find(|b| b["id"].as_i64() == Some(bd_stray))
        .expect("stray braindump found via backfill");
    assert_eq!(stray["source"], "backfill");
}

#[tokio::test]
async fn retrieve_falls_back_to_vector_direct_for_unanchored_query() {
    // ADR-0004 no-seed fallback: an unanchored query with no concept anchor
    // cannot seed; retrieval falls back to braindump-vector-direct.
    let db = Db::open_in_memory().unwrap();
    let extractor = Arc::new(SequencedExtractor {
        calls: Mutex::new(0),
        results: vec![
            ExtractionResult {
                concepts: concepts(&["Burnout"]),
                edges: vec![],
            },
            ExtractionResult {
                concepts: concepts(&["Q3 launch"]),
                edges: vec![],
            },
        ],
    });
    let app = app_with_extractor(db.clone(), extractor);
    let cookie = session_cookie(&db).await;

    let bd_reflective = submit(
        &app,
        &cookie,
        "feeling overwhelmed but my mind is full lately",
    )
    .await;
    let _bd_unrelated = submit(&app, &cookie, "the q3 launch timeline").await;

    let (status, body) = retrieve(&app, &cookie, "what is on my mind lately").await;
    assert_eq!(status, StatusCode::OK, "retrieve: {body}");
    assert_eq!(body["mode"], "no_seed_fallback");
    assert!(
        body["paths"].as_array().unwrap().is_empty(),
        "no graph traversal in fallback"
    );
    let dumps = body["braindumps"].as_array().expect("braindumps array");
    let found = dumps
        .iter()
        .find(|b| b["id"].as_i64() == Some(bd_reflective))
        .expect("reflective braindump found vector-direct");
    assert_eq!(found["source"], "vector_direct");
}

#[tokio::test]
async fn retrieve_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let request = Request::builder()
        .method("POST")
        .uri("/retrieve")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "query": "q3" }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn retrieve_rejects_empty_query() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db.clone()));
    let cookie = session_cookie(&db).await;

    let (status, body) = retrieve(&app, &cookie, "   ").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty query rejected: {body}"
    );
}
