//! Integration tests for issue #7: `DELETE /braindumps/:id` with the full
//! provenance cascade (ADR-0002 / ADR-0007 / ADR-0010). Concepts vanish when
//! their last extracting braindump is removed; edges vanish when their last
//! asserter is removed; edges whose endpoint concept vanishes are cascade-
//! deleted (ADR-0010 addendum).
//!
//! Concepts/edges are created via the real submit→extract→accrete path; auth is
//! bypassed by minting a session row directly (as in `extraction.rs`).

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
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::graph;
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// An LLM whose `extract` returns a canned result regardless of input, so the
/// accretion pipeline runs on deterministic concepts/edges. The non-extraction
/// methods are stubs (these tests drive ingest/delete, not chat/refactor).
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
        Ok("ScriptedLlm::synthesize (unused by deletion tests)".to_string())
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
/// two-braindump cycle can be driven deterministically (e.g. bd2 extracts only
/// a subset).
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
        Ok("SequencedLlm::synthesize (unused by deletion tests)".to_string())
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

fn app_with_llm(db: Db, llm: Arc<dyn Llm>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

async fn delete(app: &axum::Router, cookie: &http::HeaderValue, id: i64) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/braindumps/{id}"))
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn delete_braindump_vanishes_concept_on_last_extractor() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd1 = submit(&app, &cookie, "maria endangers q3 launch").await;
    let bd2 = submit(&app, &cookie, "maria still endangers q3 launch").await;
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let edge = graph::find_edge(&db, maria, "endangers", q3)
        .await
        .unwrap()
        .expect("edge exists");

    // Delete bd1: concept/edge still backed by bd2 → survive, provenance = [bd2].
    let (status, body) = delete(&app, &cookie, bd1).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete bd1: {body}");
    assert_eq!(
        graph::concept_provenance(&db, maria).await.unwrap(),
        vec![bd2]
    );
    assert_eq!(
        graph::edge_provenance(&db, edge.id).await.unwrap(),
        vec![bd2]
    );

    // Delete bd2: last extractor/asserter gone → concept + edge vanish.
    let (status, body) = delete(&app, &cookie, bd2).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete bd2: {body}");
    assert!(
        graph::get_concept(&db, maria).await.unwrap().is_none(),
        "concept vanishes on last extractor"
    );
    assert!(
        graph::find_edge(&db, maria, "endangers", q3)
            .await
            .unwrap()
            .is_none(),
        "edge vanishes on last asserter"
    );
}

#[tokio::test]
async fn delete_braindump_cascade_deletes_edge_when_endpoint_vanishes() {
    // ADR-0010 addendum: an edge whose endpoint concept vanishes is cascade-
    // deleted, even if another asserter still backs it. Here bd2 asserts the
    // edge without extracting Q3 (simulating a future chat-inference asserter,
    // ADR-0006), so Q3's sole extractor is bd1.
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(SequencedLlm {
        calls: Mutex::new(0),
        results: vec![
            ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch"]),
                edges: vec![edge("Maria", "endangers", "Q3 launch")],
            },
            // bd2 extracts only Maria — so Q3's sole extractor is bd1.
            ExtractionResult {
                concepts: concepts(&["Maria"]),
                edges: vec![],
            },
        ],
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd1 = submit(&app, &cookie, "maria endangers q3 launch").await;
    let bd2 = submit(&app, &cookie, "maria again").await;
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let edge = graph::find_edge(&db, maria, "endangers", q3)
        .await
        .unwrap()
        .expect("edge exists");
    // Q3 is extracted only by bd1; the edge only by bd1 so far.
    assert_eq!(graph::concept_provenance(&db, q3).await.unwrap(), vec![bd1]);
    assert_eq!(
        graph::edge_provenance(&db, edge.id).await.unwrap(),
        vec![bd1]
    );
    // Back the edge with bd2 (which did not extract Q3) so it still has an
    // asserter after bd1 is removed.
    db.run(move |conn| {
        conn.execute(
            "INSERT OR IGNORE INTO edge_provenance (edge_id, braindump_id) VALUES (?1, ?2)",
            rusqlite::params![edge.id, bd2],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let mut prov = graph::edge_provenance(&db, edge.id).await.unwrap();
    prov.sort_unstable();
    assert_eq!(prov, vec![bd1, bd2]);

    // Delete bd1: Q3's only extractor → Q3 vanishes. The edge still has bd2 as
    // an asserter, but its endpoint (Q3) is gone → cascade-deleted.
    let (status, body) = delete(&app, &cookie, bd1).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "delete bd1: {body}");
    assert!(
        graph::get_concept(&db, q3).await.unwrap().is_none(),
        "Q3 vanishes: sole extractor deleted"
    );
    assert!(
        graph::find_edge(&db, maria, "endangers", q3)
            .await
            .unwrap()
            .is_none(),
        "edge cascade-deleted: endpoint vanished"
    );
    assert!(
        graph::get_concept(&db, maria).await.unwrap().is_some(),
        "Maria survives: bd2 extracts it"
    );
}

#[tokio::test]
async fn delete_missing_braindump_is_404() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );
    let cookie = session_cookie(&db).await;

    let (status, body) = delete(&app, &cookie, 9999).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "delete missing: {body}");
}

#[tokio::test]
async fn delete_route_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = app_with_llm(
        db.clone(),
        Arc::new(ScriptedLlm {
            result: ExtractionResult::default(),
        }),
    );

    let request = Request::builder()
        .method("DELETE")
        .uri("/braindumps/1")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "delete without session"
    );
}
