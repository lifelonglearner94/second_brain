//! Integration tests for issue #11: chat write-back, structural mode
//! (ADR-0006) — `POST /chat/inferences` (propose), `GET /chat/inferences`
//! (list), `POST /chat/inferences/{id}/endorse`, `POST
//! /chat/inferences/{id}/reject`.
//!
//! The propose→HITL→endorse flow is the governed write-back surface: chat
//! proposes a structural inference (a direct edge summarizing a real
//! multi-hop edge path); the proposal enters the queue `pending` (never
//! auto-endorsed); on endorse the edge persists with `asserted_by:
//! [Chat_Inference_ID, mode: structural]`; on reject it never enters the
//! graph. Concepts + the backing path are created via the real
//! submit→extract→accrete path (a scripted extractor); auth is bypassed by
//! minting a session row directly.

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
/// methods are stubs (these tests drive inference extraction, not chat/refactor).
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
        Ok("ScriptedLlm::synthesize (unused by chat-inference tests)".to_string())
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
/// multi-braindump cycle can build the graph deterministically.
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
        Ok("SequencedLlm::synthesize (unused by chat-inference tests)".to_string())
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

async fn do_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<http::HeaderValue>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = &cookie {
        builder = builder.header(COOKIE, c);
    }
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// Seed the canonical structural path
/// `Maria —[endangers]→ Q3 launch —[depends_on]→ Beta release` and return the
/// concept ids + the app/db/cookie the tests need.
async fn seed() -> (axum::Router, Db, http::HeaderValue, i64, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch", "Beta release"]),
            edges: vec![
                edge("Maria", "endangers", "Q3 launch"),
                edge("Q3 launch", "depends_on", "Beta release"),
            ],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;
    let _bd = submit(&app, &cookie, "maria endangers q3 which beta depends on").await;
    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let beta = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta release")
        .await
        .unwrap()
        .unwrap();
    (app, db, cookie, maria, q3, beta)
}

fn propose_body(source: i64, target: i64, etype: &str, path: &[(i64, &str, i64)]) -> Value {
    json!({
        "source_concept_id": source,
        "target_concept_id": target,
        "proposed_type": etype,
        "evidence_path": path.iter().map(|(s, t, tg)| json!({
            "source_concept_id": s,
            "edge_type": t,
            "target_concept_id": tg,
        })).collect::<Vec<_>>(),
        "rationale": "the graph supports this shortcut"
    })
}

#[tokio::test]
async fn propose_with_traversable_path_creates_pending_proposal() {
    let (app, db, cookie, maria, q3, beta) = seed().await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "propose: {body}");
    assert_eq!(body["mode"], "structural_inference");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["source_concept_id"], maria);
    assert_eq!(body["target_concept_id"], beta);
    assert_eq!(body["proposed_type"], "endangers");
    assert_eq!(body["rationale"], "the graph supports this shortcut");
    let path = body["evidence_path"].as_array().expect("evidence_path");
    assert_eq!(path.len(), 2);
    // No edge persisted yet — no auto-endorse.
    assert!(
        graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .is_none(),
        "no edge persisted on a pending proposal"
    );
}

#[tokio::test]
async fn list_returns_pending_proposals() {
    let (app, _db, cookie, maria, q3, beta) = seed().await;
    do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;

    let (status, body) = do_request(&app, "GET", "/chat/inferences", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK, "list: {body}");
    let proposals = body.as_array().expect("list is an array");
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0]["status"], "pending");
    assert_eq!(proposals[0]["mode"], "structural_inference");
}

#[tokio::test]
async fn endorse_persists_edge_with_structural_inference_provenance() {
    let (app, db, cookie, maria, q3, beta) = seed().await;
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/endorse"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "endorse: {body}");
    assert_eq!(body["status"], "endorsed");

    // The direct edge persists, asserted by this inference (structural).
    let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
        .await
        .unwrap()
        .expect("endorsed edge persisted");
    let assertions = second_brain_backend::chat_inference::edge_inference_asserted_by(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        edge.id,
    )
    .await
    .unwrap();
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0].chat_inference_id, id);
    assert_eq!(assertions[0].mode, "structural_inference");
}

#[tokio::test]
async fn reject_drops_the_proposal_and_persists_no_edge() {
    let (app, db, cookie, maria, q3, beta) = seed().await;
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/reject"),
        Some(cookie.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "reject: {body}");
    assert!(
        graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .is_none(),
        "no edge persisted on reject"
    );
    // The rejected proposal is no longer pending.
    let (status, body) = do_request(&app, "GET", "/chat/inferences", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK, "list after reject: {body}");
    let proposals = body.as_array().expect("list is an array");
    assert_eq!(
        proposals[0]["status"], "rejected",
        "proposal marked rejected: {body}"
    );
}

#[tokio::test]
async fn propose_with_non_traversable_path_is_bad_request() {
    let (app, _db, cookie, maria, q3, beta) = seed().await;
    // The hop Maria —[helps]→ Q3 does not exist (the real edge is `endangers`).
    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "helps", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "non-traversable: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("not a traversable edge"),
        "{body}"
    );
}

#[tokio::test]
async fn propose_with_unsanctioned_type_is_bad_request() {
    let (app, _db, cookie, maria, q3, beta) = seed().await;
    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie),
        Some(propose_body(
            maria,
            beta,
            "bamboozles",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "unsanctioned: {body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("/ontology/propose"),
        "directed to the ontology queue: {body}"
    );
}

#[tokio::test]
async fn endorse_missing_proposal_is_404() {
    let (app, _db, cookie, _maria, _q3, _beta) = seed().await;
    let (status, _body) = do_request(
        &app,
        "POST",
        "/chat/inferences/9999/endorse",
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn reject_missing_proposal_is_404() {
    let (app, _db, cookie, _maria, _q3, _beta) = seed().await;
    let (status, _body) = do_request(
        &app,
        "POST",
        "/chat/inferences/9999/reject",
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn endorse_already_endorsed_is_conflict() {
    let (app, _db, cookie, maria, q3, beta) = seed().await;
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();
    do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/endorse"),
        Some(cookie.clone()),
        None,
    )
    .await;
    let (status, _body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/endorse"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "second endorse is conflict");
}

#[tokio::test]
async fn chat_inference_routes_require_a_session() {
    let (app, _db, _cookie, _maria, _q3, _beta) = seed().await;

    let (status, _) = do_request(&app, "GET", "/chat/inferences", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "list without session");

    let (status, _) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        None,
        Some(json!({
            "source_concept_id": 1,
            "target_concept_id": 2,
            "proposed_type": "endangers",
            "evidence_path": [{"source_concept_id": 1, "edge_type": "endangers", "target_concept_id": 2}],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "propose without session");

    let (status, _) = do_request(&app, "POST", "/chat/inferences/1/endorse", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "endorse without session");

    let (status, _) = do_request(&app, "POST", "/chat/inferences/1/reject", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "reject without session");
}

/// A second braindump cycle that extracts a direct edge Maria —[endangers]→
/// Beta, so the endorsed inference accretes onto a pre-existing edge.
#[tokio::test]
async fn endorse_accretes_onto_pre_existing_direct_edge() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(SequencedLlm {
        calls: Mutex::new(0),
        results: vec![
            // First braindump: the multi-hop path.
            ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch", "Beta release"]),
                edges: vec![
                    edge("Maria", "endangers", "Q3 launch"),
                    edge("Q3 launch", "depends_on", "Beta release"),
                ],
            },
            // Second braindump: the direct edge (Maria and Beta accrete).
            ExtractionResult {
                concepts: concepts(&["Maria", "Beta release"]),
                edges: vec![edge("Maria", "endangers", "Beta release")],
            },
        ],
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;
    let _bd1 = submit(&app, &cookie, "maria endangers q3 which beta depends on").await;
    let bd2 = submit(&app, &cookie, "maria endangers the beta release directly").await;
    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let beta = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta release")
        .await
        .unwrap()
        .unwrap();
    let pre_existing = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
        .await
        .unwrap()
        .expect("direct edge pre-exists");
    assert_eq!(
        graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, pre_existing.id)
            .await
            .unwrap(),
        vec![bd2]
    );

    // Propose + endorse the inference for the same direct edge.
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();
    let (status, _body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/endorse"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "endorse: {_body}");

    // Same edge, now asserted by both the braindump and the inference.
    let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
        .await
        .unwrap()
        .expect("edge still present");
    assert_eq!(edge.id, pre_existing.id, "edge accreted, not duplicated");
    let assertions = second_brain_backend::chat_inference::edge_inference_asserted_by(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        edge.id,
    )
    .await
    .unwrap();
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0].chat_inference_id, id);
    assert_eq!(assertions[0].mode, "structural_inference");
}

// --- issue #13: thematic inference (ADR-0006 thematic mode + ADR-0009) ---

/// Seed the canonical thematic cluster via the real submit→extract→accrete
/// path: `Maria —[endangers]→ Q3 launch —[depends_on]→ Beta release`. Louvain
/// sees one connected component; the braindump that asserted the edges is the
/// snapshot evidence. No direct Maria→Beta edge — the thematic gap.
async fn seed_thematic_cluster() -> (axum::Router, Db, http::HeaderValue, i64, i64, i64) {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch", "Beta release"]),
            edges: vec![
                edge("Maria", "endangers", "Q3 launch"),
                edge("Q3 launch", "depends_on", "Beta release"),
            ],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;
    let _bd = submit(&app, &cookie, "maria endangers q3 which beta depends on").await;
    let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let beta = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta release")
        .await
        .unwrap()
        .unwrap();
    (app, db, cookie, maria, q3, beta)
}

fn propose_thematic_body(source: i64, target: i64, etype: &str, cluster: &[i64]) -> Value {
    json!({
        "source_concept_id": source,
        "target_concept_id": target,
        "proposed_type": etype,
        "cluster_concept_ids": cluster,
        "rationale": "Louvain clustered these with no direct edge"
    })
}

#[tokio::test]
async fn propose_thematic_creates_pending_proposal_with_snapshot() {
    let (app, db, cookie, maria, q3, beta) = seed_thematic_cluster().await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie),
        Some(propose_thematic_body(
            maria,
            beta,
            "endangers",
            &[maria, q3, beta],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "propose thematic: {body}");
    assert_eq!(body["mode"], "thematic_inference");
    assert_eq!(body["status"], "pending");
    assert_eq!(body["source_concept_id"], maria);
    assert_eq!(body["target_concept_id"], beta);
    assert_eq!(body["proposed_type"], "endangers");
    assert_eq!(
        body["rationale"],
        "Louvain clustered these with no direct edge"
    );
    assert!(
        body["evidence_path"].as_array().unwrap().is_empty(),
        "thematic has no evidence path"
    );
    let snapshot = &body["snapshot"];
    assert!(
        snapshot.is_object(),
        "thematic proposal carries a snapshot: {body}"
    );
    assert!(
        !snapshot["braindump_ids"].as_array().unwrap().is_empty(),
        "snapshot has braindump evidence"
    );
    let concept_ids = snapshot["concept_ids"].as_array().unwrap();
    assert_eq!(
        concept_ids.len(),
        3,
        "snapshot captured the cluster's 3 concepts"
    );
    // No edge persisted yet — no auto-endorse.
    assert!(
        graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .is_none(),
        "no edge persisted on a pending thematic proposal"
    );
}

#[tokio::test]
async fn endorse_thematic_persists_edge_with_snapshot_in_provenance() {
    let (app, db, cookie, maria, q3, beta) = seed_thematic_cluster().await;
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie.clone()),
        Some(propose_thematic_body(
            maria,
            beta,
            "endangers",
            &[maria, q3, beta],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();
    let snapshot_id = body["snapshot"]["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/endorse"),
        Some(cookie),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "endorse thematic: {body}");
    assert_eq!(body["status"], "endorsed");

    let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
        .await
        .unwrap()
        .expect("endorsed thematic edge persisted");
    let assertions = second_brain_backend::chat_inference::edge_inference_asserted_by(
        &db,
        BOOTSTRAP_ADMIN_USER_ID,
        edge.id,
    )
    .await
    .unwrap();
    assert_eq!(assertions.len(), 1);
    assert_eq!(assertions[0].chat_inference_id, id);
    assert_eq!(assertions[0].mode, "thematic_inference");
    assert_eq!(
        assertions[0].snapshot_id,
        Some(snapshot_id),
        "snapshot attached to provenance"
    );
}

#[tokio::test]
async fn propose_thematic_rejects_endpoints_not_in_cluster() {
    let (app, _db, cookie, maria, q3, beta) = seed_thematic_cluster().await;
    // Cluster omits Beta — endpoints must be cluster-mates.
    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie),
        Some(propose_thematic_body(
            maria,
            beta,
            "endangers",
            &[maria, q3],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("cluster-mates"),
        "{body}"
    );
}

#[tokio::test]
async fn propose_thematic_rejects_unsanctioned_type() {
    let (app, _db, cookie, maria, q3, beta) = seed_thematic_cluster().await;
    let (status, body) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie),
        Some(propose_thematic_body(
            maria,
            beta,
            "bamboozles",
            &[maria, q3, beta],
        )),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("/ontology/propose"),
        "{body}"
    );
}

#[tokio::test]
async fn reject_thematic_keeps_graph_untouched() {
    let (app, db, cookie, maria, q3, beta) = seed_thematic_cluster().await;
    let (_, body) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie.clone()),
        Some(propose_thematic_body(
            maria,
            beta,
            "endangers",
            &[maria, q3, beta],
        )),
    )
    .await;
    let id = body["id"].as_i64().unwrap();

    let (status, _body) = do_request(
        &app,
        "POST",
        &format!("/chat/inferences/{id}/reject"),
        Some(cookie.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "reject thematic");
    assert!(
        graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .is_none(),
        "no edge persisted on reject"
    );
    let (status, body) = do_request(&app, "GET", "/chat/inferences", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK);
    let proposals = body.as_array().unwrap();
    assert_eq!(proposals[0]["status"], "rejected");
    assert_eq!(proposals[0]["mode"], "thematic_inference");
    assert!(
        proposals[0]["snapshot"].is_object(),
        "snapshot preserved on reject"
    );
}

#[tokio::test]
async fn list_returns_both_modes() {
    let (app, _db, cookie, maria, q3, beta) = seed_thematic_cluster().await;
    do_request(
        &app,
        "POST",
        "/chat/inferences",
        Some(cookie.clone()),
        Some(propose_body(
            maria,
            beta,
            "endangers",
            &[(maria, "endangers", q3), (q3, "depends_on", beta)],
        )),
    )
    .await;
    do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        Some(cookie.clone()),
        Some(propose_thematic_body(
            maria,
            beta,
            "helps",
            &[maria, q3, beta],
        )),
    )
    .await;

    let (status, body) = do_request(&app, "GET", "/chat/inferences", Some(cookie), None).await;
    assert_eq!(status, StatusCode::OK, "list: {body}");
    let proposals = body.as_array().unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0]["mode"], "structural_inference");
    assert_eq!(proposals[1]["mode"], "thematic_inference");
    assert!(proposals[0]["snapshot"].is_null());
    assert!(proposals[1]["snapshot"].is_object());
}

#[tokio::test]
async fn thematic_routes_require_a_session() {
    let (app, _db, _cookie, maria, q3, beta) = seed_thematic_cluster().await;
    let (status, _) = do_request(
        &app,
        "POST",
        "/chat/inferences/thematic",
        None,
        Some(propose_thematic_body(
            maria,
            beta,
            "endangers",
            &[maria, q3, beta],
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "propose thematic without session"
    );
}
