//! Integration tests for issue #12's chat half (ADR-0008): the current thematic
//! partition is layered into chat as macrostructure context, the LLM may use it
//! as a magnifying glass but never cite it, and retrieval stays seed-then-expand
//! along typed edges (cluster membership is never a traversal mechanism).
//!
//! A two-cluster graph (two disjoint typed edges) is built via the real
//! submit→extract→accrete path with a sequenced extractor so each braindump
//! feeds only one cluster. The LLM seam is a scripted stand-in that records the
//! system prompt it received, so the macrostructure-context integration is
//! asserted against the actual prompt sent to the model — hermetic, no Gemini.

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
    ExtractedConcept, ExtractedEdge, ExtractionResult,
};
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The scripted LLM for the chat-macrostructure tests (issue #39 collapsed the
/// former separate `SequencedExtractor` + `RecordingLlm` into one struct):
/// `extract` returns a canned result per call in sequence (so two braindumps
/// feed two disjoint clusters — bd1 → Maria/Q3, bd2 → Alpha/Beta), and
/// `synthesize` records each system prompt and returns a canned answer (so
/// tests can assert the macrostructure context reached the model).
struct ScriptedLlm {
    extract_calls: Mutex<usize>,
    extraction_results: Vec<ExtractionResult>,
    answer: String,
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Llm for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, system: &str, _user: &str) -> Result<String> {
        self.prompts.lock().unwrap().push(system.to_string());
        Ok(self.answer.clone())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        let mut calls = self.extract_calls.lock().unwrap();
        let idx = *calls;
        *calls += 1;
        Ok(self.extraction_results.get(idx).cloned().unwrap_or_default())
    }
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(text, 64))
    }
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(text, 64))
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

async fn chat(app: &axum::Router, cookie: &http::HeaderValue, query: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method("POST")
        .uri("/chat")
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

fn two_cluster_app(
    db: Db,
    answer: String,
    prompts: Arc<Mutex<Vec<String>>>,
) -> axum::Router {
    let extraction_results = vec![
        ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
        ExtractionResult {
            concepts: concepts(&["Alpha", "Beta"]),
            edges: vec![edge("Alpha", "helps", "Beta")],
        },
    ];
    let llm: Arc<dyn Llm> = Arc::new(ScriptedLlm {
        extract_calls: Mutex::new(0),
        extraction_results,
        answer,
        prompts,
    });
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

#[tokio::test]
async fn chat_prompt_layers_the_partition_as_macrostructure_context() {
    // ADR-0008: chat consumes the current partition as macrostructure context.
    // The recorded system prompt must carry the partition (session labels +
    // every concept) so the LLM can see thematic structure it could not derive
    // from the raw edges in-budget.
    let db = Db::open_in_memory().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let app = two_cluster_app(
        db.clone(),
        String::from(
            "Q3 is at risk because Maria is leaving [bd:1] \
             [edge:Maria —endangers→ Q3 launch]",
        ),
        prompts.clone(),
    );
    let cookie = session_cookie(&db).await;
    let bd1 = submit(&app, &cookie, "maria endangers q3 launch").await;
    let bd2 = submit(&app, &cookie, "alpha helps beta").await;
    let _ = bd2;

    let (status, body) = chat(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], false, "grounded, not silent");

    let prompt = prompts
        .lock()
        .unwrap()
        .last()
        .cloned()
        .expect("the LLM was called and recorded its prompt");
    assert!(
        prompt.contains("Macrostructure context"),
        "macrostructure section layered into the prompt: {prompt}"
    );
    assert!(
        prompt.contains("Group 1 for this session") && prompt.contains("Group 2 for this session"),
        "ADR-0008 ephemeral session labels in the prompt: {prompt}"
    );
    assert!(
        prompt.contains("Maria")
            && prompt.contains("Q3 launch")
            && prompt.contains("Alpha")
            && prompt.contains("Beta"),
        "every concept label appears in the macrostructure: {prompt}"
    );
    let _ = bd1;
}

#[tokio::test]
async fn chat_rejects_an_answer_that_cites_a_cluster() {
    // ADR-0008 + the structural backstop: an answer that cites a cluster
    // (`[cluster:...]`) rests on an ephemeral projection, not the stable truth.
    // The endpoint must reject it — returning silence with no citations — so the
    // frontend never shows sources for an ungrounded answer. This is the
    // "citation-from-cluster case rejected".
    let db = Db::open_in_memory().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let app = two_cluster_app(
        db.clone(),
        String::from(
            "Q3 is at risk because Group 1 is dense \
             [cluster:Group 1] [bd:1]",
        ),
        prompts.clone(),
    );
    let cookie = session_cookie(&db).await;
    submit(&app, &cookie, "maria endangers q3 launch").await;
    submit(&app, &cookie, "alpha helps beta").await;

    let (status, body) = chat(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], true, "cluster-citing answer rejected");
    assert_eq!(
        body["answer"].as_str().unwrap(),
        "you haven't told me about that",
        "rejection returns the ADR-0005 silence phrasing"
    );
    assert!(
        body["citations"].as_array().unwrap().is_empty(),
        "no citations shown for a rejected answer"
    );
    assert!(
        body["paths"].as_array().unwrap().is_empty(),
        "no paths shown for a rejected answer"
    );
    assert_eq!(
        prompts.lock().unwrap().len(),
        1,
        "the LLM was called once before the guard rejected the answer"
    );
}

#[tokio::test]
async fn chat_with_macrostructure_still_cites_braindumps_and_edges() {
    // The partition layers in without breaking ADR-0005's citation contract: a
    // grounded answer that cites braindumps + edges is returned with its
    // citations, even though the macrostructure context is in the prompt.
    let db = Db::open_in_memory().unwrap();
    let prompts = Arc::new(Mutex::new(Vec::<String>::new()));
    let app = two_cluster_app(
        db.clone(),
        String::from(
            "Q3 is at risk because Maria is leaving, which endangers the launch \
             [bd:1] [edge:Maria —endangers→ Q3 launch]",
        ),
        prompts.clone(),
    );
    let cookie = session_cookie(&db).await;
    let bd = submit(&app, &cookie, "maria endangers q3 launch").await;
    submit(&app, &cookie, "alpha helps beta").await;

    let (status, body) = chat(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], false, "grounded, not silent");
    let citations = body["citations"].as_array().expect("citations array");
    assert!(
        citations.iter().any(|c| c["id"].as_i64() == Some(bd)),
        "the Maria braindump is cited: {citations:?}"
    );
    let paths = body["paths"].as_array().expect("paths array");
    assert!(
        paths.iter().any(|e| {
            e["source_concept_label"] == "Maria"
                && e["target_concept_label"] == "Q3 launch"
                && e["edge_type"] == "endangers"
        }),
        "the `endangers` edge is in the response paths: {paths:?}"
    );
    assert!(
        prompts
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("Macrostructure context"),
        "the macrostructure was layered in alongside the citations"
    );
}

#[tokio::test]
async fn retrieval_does_not_use_cluster_membership() {
    // ADR-0008: retrieval stays strictly seed-then-expand along typed edges
    // (plus vector backfill); cluster membership is never a traversal mechanism.
    // On a two-cluster graph, retrieval seeded on the Q3 cluster reaches the
    // Maria braindump via the typed edge (source = subgraph), runs in
    // seed-then-expand mode, and carries no cluster field. The other cluster's
    // braindump may appear via vector backfill (a legitimate ADR-0004
    // mechanism) but never via a cluster-based source.
    let db = Db::open_in_memory().unwrap();
    let app = two_cluster_app(
        db.clone(),
        String::from("unused"),
        Arc::new(Mutex::new(Vec::new())),
    );
    let cookie = session_cookie(&db).await;
    let bd_maria = submit(&app, &cookie, "maria endangers q3 launch").await;
    let bd_alpha = submit(&app, &cookie, "alpha helps beta").await;

    let (status, body) = retrieve(&app, &cookie, "Q3").await;
    assert_eq!(status, StatusCode::OK, "retrieve: {body}");
    assert_eq!(
        body["mode"], "seed_then_expand",
        "retrieval used seed-then-expand (typed edges), not a cluster mode"
    );
    assert!(
        body.get("clusters").is_none() && body.get("cluster").is_none(),
        "retrieval carries no cluster field — clusters are not a retrieval concept"
    );
    let braindumps = body["braindumps"].as_array().expect("braindumps array");
    let maria = braindumps
        .iter()
        .find(|b| b["id"].as_i64() == Some(bd_maria))
        .expect("the Q3 cluster's braindump is reached via the typed edge");
    assert_eq!(
        maria["source"], "subgraph",
        "reached by graph traversal, not by cluster membership"
    );
    if let Some(alpha) = braindumps
        .iter()
        .find(|b| b["id"].as_i64() == Some(bd_alpha))
    {
        assert_ne!(
            alpha["source"], "subgraph",
            "the other cluster's braindump is never reached via graph traversal (no cluster-jumping): {alpha:?}"
        );
    }
}
