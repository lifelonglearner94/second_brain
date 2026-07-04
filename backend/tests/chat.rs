//! Integration tests for issue #10: the chat read surface (ADR-0005).
//!
//! `POST /chat` runs the retrieval read path (ADR-0004), then synthesizes over
//! the retrieved braindumps + traversed edge paths under a grounded-synthesis
//! system prompt. Every claim cites braindump ids + edge refs; inference weaves
//! only along edges that actually exist; when the graph doesn't support an
//! answer, chat is silent (ADR-0005). Auth is bypassed by minting a session row
//! directly (as in `tests/retrieval.rs`); the extractor is a scripted stand-in
//! so the accretion pipeline runs on deterministic concepts/edges — no Gemini
//! call. The LLM seam is a scripted stand-in so synthesis is hermetic.

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
use second_brain_backend::llm::Llm;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// A scripted LLM for the chat tests: `extract` returns a canned extraction
/// result so the accretion pipeline builds a deterministic graph, `synthesize`
/// returns a canned answer and records each call so tests can assert the
/// silence path skips the LLM. The remaining methods are stubs.
#[derive(Clone)]
struct ScriptedLlm {
    extraction: ExtractionResult,
    answer: String,
    calls: Arc<Mutex<usize>>,
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
        *self.calls.lock().unwrap() += 1;
        Ok(self.answer.clone())
    }
    async fn extract(
        &self,
        _verbatim: &str,
        _ontology_slugs: &[String],
    ) -> Result<ExtractionResult> {
        Ok(self.extraction.clone())
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

fn app_with(db: Db, llm: Arc<dyn Llm>) -> axum::Router {
    let mut state = AppState::for_tests(db);
    state.llm = llm;
    routes::router(state)
}

#[tokio::test]
async fn chat_is_silent_when_the_graph_does_not_support_an_answer() {
    // ADR-0005 silence contract: an empty graph cannot support any answer, so
    // chat returns "you haven't told me about that" — without calling the LLM
    // (silence is enforced structurally, not entrusted to the model).
    let db = Db::open_in_memory().unwrap();
    let llm_calls = Arc::new(Mutex::new(0usize));
    let llm = Arc::new(ScriptedLlm {
        extraction: ExtractionResult::default(),
        answer: String::from("Q3 is at risk because Maria is leaving [bd:1]"),
        calls: llm_calls.clone(),
    });
    let app = app_with(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let (status, body) = chat(&app, &cookie, "why is Q3 at risk?").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], true, "silent when no support");
    assert_eq!(
        body["answer"].as_str().unwrap(),
        "you haven't told me about that",
        "ADR-0005 silence phrasing"
    );
    assert!(
        body["citations"].as_array().unwrap().is_empty(),
        "no citations when silent"
    );
    assert!(
        body["paths"].as_array().unwrap().is_empty(),
        "no paths when silent"
    );
    assert_eq!(
        *llm_calls.lock().unwrap(),
        0,
        "LLM never called on the silence path"
    );
}

#[tokio::test]
async fn chat_synthesizes_a_grounded_answer_citing_braindumps_and_edges() {
    // ADR-0005 demo: "why is Q3 at risk?" → a grounded answer citing the Maria
    // braindump + the `endangers` edge. Retrieval seeds on Q3 launch, traverses
    // the incoming `endangers` edge to Maria, collects her braindump; the
    // scripted LLM returns a grounded synthesis; the response carries the
    // retrieved braindumps as citations and the traversed edge as a path so the
    // frontend can render drill-downable sources.
    let db = Db::open_in_memory().unwrap();
    let llm_calls = Arc::new(Mutex::new(0usize));
    let llm = Arc::new(ScriptedLlm {
        extraction: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
        answer: String::from(
            "Q3 is at risk because Maria is leaving, which endangers the launch \
             [bd:1] [edge:Maria —endangers→ Q3 launch]",
        ),
        calls: llm_calls.clone(),
    });
    let app = app_with(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd = submit(&app, &cookie, "maria leaving tanks the timeline").await;

    let (status, body) = chat(&app, &cookie, "why is Q3 at risk?").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], false, "grounded, not silent");
    assert_eq!(
        body["answer"].as_str().unwrap(),
        "Q3 is at risk because Maria is leaving, which endangers the launch \
         [bd:1] [edge:Maria —endangers→ Q3 launch]",
        "answer is the grounded synthesis"
    );
    let citations = body["citations"].as_array().expect("citations array");
    let cited = citations
        .iter()
        .find(|c| c["id"].as_i64() == Some(bd))
        .expect("the Maria braindump is cited");
    assert_eq!(cited["source"], "subgraph", "cited via graph traversal");
    let paths = body["paths"].as_array().expect("paths array");
    assert!(
        paths.iter().any(|e| {
            e["source_concept_label"] == "Maria"
                && e["target_concept_label"] == "Q3 launch"
                && e["edge_type"] == "endangers"
        }),
        "the `endangers` edge is in the response paths: {paths:?}"
    );
    assert_eq!(
        *llm_calls.lock().unwrap(),
        1,
        "LLM called once on the grounded path"
    );
}

#[tokio::test]
async fn chat_is_silent_when_the_llm_judges_the_context_does_not_support_an_answer() {
    // ADR-0005 silence can also be LLM-judged: retrieval returned braindumps,
    // but they don't actually answer the query. The system prompt instructs
    // the model to return the silence phrasing; the endpoint must reflect that
    // as silence (no citations) so the frontend never shows sources for an
    // answer that doesn't exist.
    let db = Db::open_in_memory().unwrap();
    let llm_calls = Arc::new(Mutex::new(0usize));
    let llm = Arc::new(ScriptedLlm {
        extraction: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
        answer: String::from("you haven't told me about that"),
        calls: llm_calls.clone(),
    });
    let app = app_with(db.clone(), llm);
    let cookie = session_cookie(&db).await;
    let _bd = submit(&app, &cookie, "maria leaving tanks the timeline").await;

    let (status, body) = chat(&app, &cookie, "what's the weather forecast?").await;
    assert_eq!(status, StatusCode::OK, "chat: {body}");
    assert_eq!(body["silent"], true, "LLM-judged silence reflected");
    assert_eq!(
        body["answer"].as_str().unwrap(),
        "you haven't told me about that",
        "ADR-0005 silence phrasing"
    );
    assert!(
        body["citations"].as_array().unwrap().is_empty(),
        "no citations when the LLM goes silent"
    );
    assert!(
        body["paths"].as_array().unwrap().is_empty(),
        "no paths when the LLM goes silent"
    );
    assert_eq!(
        *llm_calls.lock().unwrap(),
        1,
        "LLM was called (the context was non-empty) before judging silence"
    );
}

#[tokio::test]
async fn chat_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let request = Request::builder()
        .method("POST")
        .uri("/chat")
        .header("content-type", "application/json")
        .body(Body::from(json!({ "query": "q3" }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chat_rejects_empty_query() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db.clone()));
    let cookie = session_cookie(&db).await;

    let (status, body) = chat(&app, &cookie, "   ").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty query rejected: {body}"
    );
}
