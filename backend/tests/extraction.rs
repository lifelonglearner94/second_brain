//! Integration tests for issue #6: the submit route drives extraction → atomic
//! accretion (identity + provenance + type history + embeddings) end-to-end.
//!
//! Auth is bypassed by minting a session row directly (as in `braindump.rs`).
//! The extractor is a scripted stand-in returning canned concepts/edges so the
//! test is hermetic - no Gemini call. The accretion it triggers is the real
//! pipeline (`graph::ingest_extraction`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::db::BOOTSTRAP_ADMIN_USER_ID;
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
/// methods are stubs (these tests drive ingest, not chat/refactor).
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
        Ok("ScriptedLlm::synthesize (unused by extraction tests)".to_string())
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

/// A scripted LLM whose `extract` returns a *different* result on each call,
/// so a single submit→edit cycle can be driven deterministically.
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
        Ok("SequencedLlm::synthesize (unused by extraction tests)".to_string())
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

#[tokio::test]
async fn submit_drives_extraction_and_accretion_end_to_end() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd = submit(&app, &cookie, "maria endangers the q3 launch").await;

    // Both concepts landed with this braindump in their extraction provenance.
    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
            .await
            .unwrap(),
        vec![bd],
        "extraction provenance (ADR-0010)"
    );
    assert_eq!(
        graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, q3)
            .await
            .unwrap(),
        vec![bd]
    );

    // The edge landed with type_history at index 0 = the original assertion.
    let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
        .await
        .unwrap()
        .expect("edge created");
    assert_eq!(edge.original_type, "endangers");
    assert_eq!(
        graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap(),
        vec![bd]
    );
    let history = graph::edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].seq_index, 0);
    assert_eq!(history[0].type_slug, "endangers");

    // Both embeddings persisted (Gemini in prod; FakeLlm here).
    assert!(
        graph::braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn two_braindumps_same_concept_accrete_into_one_node_via_route() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Q3 review"]),
            edges: vec![],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd1 = submit(&app, &cookie, "the q3 review went off the rails").await;
    let bd2 = submit(&app, &cookie, "q3 review is still on my mind").await;

    // One concept node (identical label → identical FakeLlm vector →
    // cosine 1.0 > 0.95 → accrete), both braindumps in its provenance.
    let cid = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 review")
        .await
        .unwrap()
        .unwrap();
    let mut prov = graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
        .await
        .unwrap();
    prov.sort_unstable();
    assert_eq!(prov, vec![bd1, bd2]);
}

#[tokio::test]
async fn second_braindump_accretes_edge_provenance_via_route() {
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

    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
        .await
        .unwrap()
        .expect("edge exists");
    // ADR-0002: the second assertion accretes to asserted_by, no new edge.
    let mut prov = graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
        .await
        .unwrap();
    prov.sort_unstable();
    assert_eq!(prov, vec![bd1, bd2]);
    // Still a single type-history entry (no second assertion appended a type).
    assert_eq!(
        graph::edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn unsanctioned_edge_type_rejected_via_route() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "bamboozles", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    submit(&app, &cookie, "maria bamboozles q3 launch").await;

    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    // Concepts were created, but the unsanctioned edge was rejected.
    assert!(
        graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "bamboozles", q3)
            .await
            .unwrap()
            .is_none(),
        "unsanctioned edge type must not persist"
    );
}

#[tokio::test]
async fn edit_retracts_stale_extraction_and_re_accretes_via_route() {
    // ADR-0007: an edit re-extracts; the old derived graph is retracted first.
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(SequencedLlm {
        calls: Mutex::new(0),
        results: vec![
            ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch"]),
                edges: vec![edge("Maria", "endangers", "Q3 launch")],
            },
            // The edit re-extracts a different concept set.
            ExtractionResult {
                concepts: concepts(&["Alpha project"]),
                edges: vec![],
            },
        ],
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd = submit(&app, &cookie, "maria endangers q3 launch").await;
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .is_some()
    );

    // PATCH: error-correct the verbatim; the extractor returns the second
    // scripted result on the re-extraction.
    let request = Request::builder()
        .method("PATCH")
        .uri(format!("/braindumps/{bd}"))
        .header(COOKIE, &cookie)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({ "verbatim": "actually about the alpha project" }).to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // The stale Maria/Q3 concepts (only this braindump asserted them) are gone;
    // the new Alpha concept is present.
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .is_none(),
        "stale concept retracted on edit"
    );
    assert!(
        graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Alpha project")
            .await
            .unwrap()
            .is_some(),
        "new concept accreted on edit"
    );
    assert_eq!(
        graph::concept_provenance(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Alpha project")
                .await
                .unwrap()
                .unwrap()
        )
        .await
        .unwrap(),
        vec![bd],
        "the edited braindump is the new concept's sole extractor"
    );
}
