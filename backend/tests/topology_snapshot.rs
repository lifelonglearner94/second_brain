//! Integration tests for issue #27: the Global Topology Snapshot endpoint
//! `GET /graph` — the primary read surface for visualization. Returns all
//! concepts + typed edges (current type projected from type_history,
//! ADR-0003) + current Louvain partition IDs (ADR-0008) as a single gzipped
//! JSON payload the frontend fetches wholesale on app load. The backend owns
//! all graph computation, including the partition IDs — the frontend never
//! runs Louvain (ADR-0008).
//!
//! Concepts/edges are created via the real submit→extract→accrete path; auth
//! is bypassed by minting a session row directly (as in `delta_sync.rs`). The
//! extractor is a scripted stand-in so the accretion pipeline runs on
//! deterministic concepts/edges — no Gemini call.

use std::io::Read;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use flate2::read::GzDecoder;
use http::header::{CONTENT_ENCODING, CONTENT_TYPE, COOKIE};
use http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::error::Result;
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::graph;
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// An LLM whose `extract` returns a canned result regardless of input, so the
/// accretion pipeline runs on deterministic concepts/edges. The non-extraction
/// methods are stubs (these tests drive ingest + snapshot reads, not chat).
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
        Ok("ScriptedLlm::synthesize (unused by topology snapshot tests)".to_string())
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

/// Append a retag entry (seq_index = max+1) to an edge's type history —
/// simulates the async ontology refactor (ADR-0003) without standing up
/// governance.
async fn append_retag(db: &Db, edge_id: i64, type_slug: &str) {
    let type_slug = type_slug.to_string();
    db.with_conn_test(move |conn| {
        let next_seq: i64 = conn.query_row(
            "SELECT COALESCE(MAX(seq_index), -1) + 1 FROM edge_type_history WHERE edge_id = ?1",
            rusqlite::params![edge_id],
            |r| r.get(0),
        )?;
        let now = second_brain_backend::db::now_seconds();
        conn.execute(
            "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![edge_id, next_seq, type_slug, now],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// GET /graph, returning status, decoded JSON, and the response headers. The
/// body is gunzipped when the response carries `Content-Encoding: gzip`.
async fn graph(app: &axum::Router, cookie: &http::HeaderValue) -> (StatusCode, Value, HeaderMap) {
    let request = Request::builder()
        .method("GET")
        .uri("/graph")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json_bytes = if headers
        .get(CONTENT_ENCODING)
        .map(|v| v.to_str().unwrap_or(""))
        == Some("gzip")
    {
        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut out = Vec::new();
        decoder.read_to_end(&mut out).expect("gzip decode");
        out
    } else {
        bytes.to_vec()
    };
    let value: Value = serde_json::from_slice(&json_bytes).unwrap_or(Value::Null);
    (status, value, headers)
}

fn app_with_llm(db: Db, llm: Arc<dyn Llm>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

fn partition_id_of(body: &Value, concept_id: i64) -> i64 {
    body["partitions"]
        .as_array()
        .expect("partitions array")
        .iter()
        .find(|p| p["concept_id"].as_i64() == Some(concept_id))
        .map(|p| p["partition_id"].as_i64().expect("partition_id"))
        .unwrap_or_else(|| panic!("concept {concept_id} has a partition assignment"))
}

#[tokio::test]
async fn get_graph_returns_gzipped_snapshot_with_concepts_edges_and_partitions() {
    // Two disjoint edges → two Louvain communities. The endpoint returns the
    // full snapshot (all concepts, both edges with projected current type,
    // a partition assignment per concept) as a gzipped JSON payload.
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

    let (status, body, headers) = graph(&app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "graph: {body}");
    assert_eq!(
        headers.get(CONTENT_ENCODING).map(|v| v.to_str().unwrap()),
        Some("gzip"),
        "payload is gzipped (Content-Encoding: gzip)"
    );
    assert_eq!(
        headers.get(CONTENT_TYPE).map(|v| v.to_str().unwrap()),
        Some("application/json"),
        "content-type is JSON"
    );

    let concepts_arr = body["concepts"].as_array().expect("concepts array");
    assert_eq!(concepts_arr.len(), 4, "all four concepts");
    let labels: Vec<&str> = concepts_arr
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    for wanted in &["Maria", "Q3 launch", "Alpha", "Beta"] {
        assert!(labels.contains(wanted), "{wanted} in concepts: {labels:?}");
    }

    let edges = body["edges"].as_array().expect("edges array");
    assert_eq!(edges.len(), 2, "both edges");
    for e in edges {
        assert_eq!(
            e["current_type"], e["original_type"],
            "fresh edge: current type projected == original"
        );
    }

    let partitions = body["partitions"].as_array().expect("partitions array");
    assert_eq!(partitions.len(), 4, "every concept assigned a partition id");
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let alpha = graph::concept_id_for_label(&db, "Alpha")
        .await
        .unwrap()
        .unwrap();
    let beta = graph::concept_id_for_label(&db, "Beta")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        partition_id_of(&body, maria),
        partition_id_of(&body, q3),
        "Maria and Q3 in the same community"
    );
    assert_eq!(
        partition_id_of(&body, alpha),
        partition_id_of(&body, beta),
        "Alpha and Beta in the same community"
    );
    assert_ne!(
        partition_id_of(&body, maria),
        partition_id_of(&body, alpha),
        "the two communities have distinct partition ids"
    );
}

#[tokio::test]
async fn get_graph_projects_current_type_from_type_history_after_retag() {
    // ADR-0003: the snapshot's edge type is the projected current type, not a
    // stored field. After a refactor appends "supports" to a "helps" edge's
    // history, the snapshot reports current_type = "supports" with
    // original_type = "helps".
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch"]),
                edges: vec![edge("Maria", "helps", "Q3 launch")],
            },
        }),
    );
    let cookie = session_cookie(&db).await;
    submit(&app, &cookie, "maria helps q3 launch").await;
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let edge_row = graph::find_edge(&db, maria, "helps", q3)
        .await
        .unwrap()
        .expect("edge exists");
    append_retag(&db, edge_row.id, "supports").await;

    let (status, body, _headers) = graph(&app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "graph: {body}");
    let e = body["edges"]
        .as_array()
        .expect("edges array")
        .iter()
        .find(|e| e["id"].as_i64() == Some(edge_row.id))
        .expect("edge in snapshot");
    assert_eq!(e["original_type"], "helps", "original assertion immutable");
    assert_eq!(e["current_type"], "supports", "current type projected");
}

#[tokio::test]
async fn get_graph_on_empty_graph_returns_empty_gzipped_snapshot() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );
    let cookie = session_cookie(&db).await;

    let (status, body, headers) = graph(&app, &cookie).await;
    assert_eq!(status, StatusCode::OK, "graph: {body}");
    assert_eq!(
        headers.get(CONTENT_ENCODING).map(|v| v.to_str().unwrap()),
        Some("gzip"),
        "empty snapshot still gzipped"
    );
    assert_eq!(body["concepts"].as_array().unwrap().len(), 0);
    assert_eq!(body["edges"].as_array().unwrap().len(), 0);
    assert_eq!(body["partitions"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn get_graph_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let request = Request::builder()
        .method("GET")
        .uri("/graph")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "snapshot without session is rejected"
    );
}
