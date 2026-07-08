//! Integration test for issue #72: per-user graph isolation.
//!
//! Two users with separate minted sessions each submit a Braindump; each
//! user's Global Topology Snapshot, Delta Sync, Chat, and merge-suggestion
//! endpoints expose only their own Concepts/Edges. A request by user A for
//! user B's Braindump by id returns 404.

use std::sync::Arc;

use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use second_brain_backend::graph_repo::SqliteGraphRepo;
use second_brain_backend::llm::Llm;
use second_brain_backend::{routes, state::AppState};
use serde_json::{json, Value};
use tower::ServiceExt;

const ADMIN_ID: &str = "00000000-0000-0000-0000-000000000001";
const USER_B_ID: &str = "00000000-0000-0000-0000-000000000002";

/// Mint a session for `user_id` and return the cookie header value.
async fn session_cookie(db: &Db, user_id: &str) -> http::HeaderValue {
    let session = mint_session(db, user_id).await.unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

/// Submit a braindump via POST /braindumps with the given cookie.
async fn submit_braindump(app: &axum::Router, cookie: &http::HeaderValue, verbatim: &str) -> Value {
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
    assert_eq!(status, StatusCode::OK, "submit braindump: {value}");
    value
}

/// GET /graph (topology snapshot) — decompress gzip and parse JSON.
async fn get_graph(app: &axum::Router, cookie: &http::HeaderValue) -> Value {
    let request = Request::builder()
        .uri("/graph")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
    let mut json_str = String::new();
    decoder.read_to_string(&mut json_str).unwrap();
    serde_json::from_str(&json_str).unwrap()
}

/// GET /graph/delta?since=0
async fn get_delta(app: &axum::Router, cookie: &http::HeaderValue) -> Value {
    let request = Request::builder()
        .uri("/graph/delta?since=0")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// GET /merge-suggestions
async fn get_merge_suggestions(app: &axum::Router, cookie: &http::HeaderValue) -> Value {
    let request = Request::builder()
        .uri("/merge-suggestions")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// POST /chat
async fn post_chat(app: &axum::Router, cookie: &http::HeaderValue, query: &str) -> Value {
    let request = Request::builder()
        .method("POST")
        .uri("/chat")
        .header(COOKIE, cookie)
        .header("content-type", "application/json")
        .body(Body::from(json!({ "query": query }).to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK, "chat response");
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

/// GET /braindumps/{id}
async fn get_braindump(
    app: &axum::Router,
    cookie: &http::HeaderValue,
    id: i64,
) -> (StatusCode, Value) {
    let request = Request::builder()
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

/// An LLM that extracts a fixed concept+edge set from any braindump, so both
/// users get graph data. The concept labels are user-specific so we can verify
/// isolation.
struct ScriptedLlm {
    result: ExtractionResult,
}

#[async_trait::async_trait]
impl Llm for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> second_brain_backend::error::Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(
        &self,
        _: &str,
        user: &str,
    ) -> second_brain_backend::error::Result<String> {
        Ok(user.to_string())
    }
    async fn synthesize(&self, _: &str, _: &str) -> second_brain_backend::error::Result<String> {
        Ok("synthesized answer".to_string())
    }
    async fn extract(
        &self,
        _: &str,
        _: &[String],
    ) -> second_brain_backend::error::Result<ExtractionResult> {
        Ok(self.result.clone())
    }
    async fn embed_document(&self, text: &str) -> second_brain_backend::error::Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    async fn embed_query(&self, text: &str) -> second_brain_backend::error::Result<Vec<f32>> {
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, 64,
        ))
    }
    fn dim(&self) -> usize {
        64
    }
}

/// Build AppState with a scripted LLM that extracts the given concepts+edges.
fn app_with_extraction(result: ExtractionResult) -> (axum::Router, Db) {
    let db = Db::open_in_memory().unwrap();
    let llm: Arc<dyn Llm> = Arc::new(ScriptedLlm { result });
    db.ensure_vec_tables(llm.dim()).unwrap();
    let webauthn =
        second_brain_backend::auth::build_webauthn("localhost", "http://localhost:8080", "test")
            .unwrap();
    let graph_repo: Arc<dyn second_brain_backend::graph_repo::GraphRepo> =
        Arc::new(SqliteGraphRepo::new(db.clone()));
    let state = AppState {
        db: db.clone(),
        config: Arc::new(second_brain_backend::config::Config::for_tests()),
        llm,
        auth: second_brain_backend::auth::AuthService::new(webauthn),
        log_buffer: second_brain_backend::logs::LogBuffer::with_default_capacity(),
        refactor_runner: second_brain_backend::ontology::RefactorRunner::new(),
        ingest_runner: second_brain_backend::braindump::IngestRunner::new_inline(),
        graph_repo,
    };
    (routes::router(state), db)
}

fn extraction(concepts: &[&str], edges: &[(&str, &str, &str)]) -> ExtractionResult {
    ExtractionResult {
        concepts: concepts
            .iter()
            .map(|l| ExtractedConcept {
                label: l.to_string(),
            })
            .collect(),
        edges: edges
            .iter()
            .map(|(s, t, tg)| ExtractedEdge {
                from_label: s.to_string(),
                type_slug: t.to_string(),
                to_label: tg.to_string(),
            })
            .collect(),
    }
}

#[tokio::test]
async fn two_users_are_isolated() {
    // Both users submit braindumps; each user's graph reads (topology snapshot,
    // delta sync, chat, merge-suggestions) expose only their own concepts/edges.
    // A request by user A for user B's braindump by id returns 404.
    let (app, db) = app_with_extraction(extraction(
        &["Alpha", "Beta"],
        &[("Alpha", "endangers", "Beta")],
    ));

    // Seed ontology for both users (the bootstrap admin is already seeded by
    // the migration; user B needs a users row + ontology seeding).
    db.with_conn_test(|conn| {
        conn.execute(
            "INSERT INTO users (id, display_name, is_admin, created_at)
             VALUES (?1, 'user_b', 0, unixepoch())",
            rusqlite::params![USER_B_ID],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    second_brain_backend::db::seed_ontology_for_user(&db, USER_B_ID)
        .await
        .unwrap();

    // Mint sessions for both users.
    let cookie_a = session_cookie(&db, ADMIN_ID).await;
    let cookie_b = session_cookie(&db, USER_B_ID).await;

    // Each user submits a braindump.
    let bd_a = submit_braindump(&app, &cookie_a, "alpha endangers beta").await;
    let bd_b = submit_braindump(&app, &cookie_b, "alpha endangers beta").await;
    let bd_a_id = bd_a["id"].as_i64().unwrap();
    let bd_b_id = bd_b["id"].as_i64().unwrap();

    // --- Topology Snapshot isolation ---
    let graph_a = get_graph(&app, &cookie_a).await;
    let graph_b = get_graph(&app, &cookie_b).await;
    let concepts_a = graph_a["concepts"].as_array().unwrap();
    let concepts_b = graph_b["concepts"].as_array().unwrap();
    assert_eq!(
        concepts_a.len(),
        2,
        "user A sees exactly their 2 concepts: {concepts_a:?}"
    );
    assert_eq!(
        concepts_b.len(),
        2,
        "user B sees exactly their 2 concepts: {concepts_b:?}"
    );
    // The concept IDs must differ between users (separate rows).
    let a_ids: Vec<i64> = concepts_a
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    let b_ids: Vec<i64> = concepts_b
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert!(
        a_ids.iter().all(|id| !b_ids.contains(id)),
        "user A's concept ids must not appear in user B's graph: {a_ids:?} vs {b_ids:?}"
    );
    // Each user has exactly 1 edge.
    assert_eq!(graph_a["edges"].as_array().unwrap().len(), 1);
    assert_eq!(graph_b["edges"].as_array().unwrap().len(), 1);

    // --- Delta Sync isolation ---
    let delta_a = get_delta(&app, &cookie_a).await;
    let delta_b = get_delta(&app, &cookie_b).await;
    assert_eq!(
        delta_a["added_concepts"].as_array().unwrap().len(),
        2,
        "user A delta has 2 concepts"
    );
    assert_eq!(
        delta_b["added_concepts"].as_array().unwrap().len(),
        2,
        "user B delta has 2 concepts"
    );

    // --- Chat isolation ---
    let chat_a = post_chat(&app, &cookie_a, "Alpha").await;
    let chat_b = post_chat(&app, &cookie_b, "Alpha").await;
    // Each user's chat retrieves only their own braindumps.
    let citations_a = chat_a["citations"].as_array().unwrap();
    let citations_b = chat_b["citations"].as_array().unwrap();
    let a_bd_ids: Vec<i64> = citations_a
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    let b_bd_ids: Vec<i64> = citations_b
        .iter()
        .map(|c| c["id"].as_i64().unwrap())
        .collect();
    assert!(
        a_bd_ids.iter().all(|id| !b_bd_ids.contains(id)),
        "user A's braindump ids must not appear in user B's chat: {a_bd_ids:?} vs {b_bd_ids:?}"
    );

    // --- Merge-suggestions isolation ---
    let suggestions_a = get_merge_suggestions(&app, &cookie_a).await;
    let suggestions_b = get_merge_suggestions(&app, &cookie_b).await;
    // Both users have the same extraction → no merge suggestions (exact match
    // → accretion, not suggestion). But the lists are separate — an empty list
    // for one user doesn't mean the other's would leak.
    assert!(
        suggestions_a.as_array().is_some(),
        "user A merge-suggestions is a list"
    );
    assert!(
        suggestions_b.as_array().is_some(),
        "user B merge-suggestions is a list"
    );

    // --- Cross-user braindump access returns 404 ---
    let (status, _body) = get_braindump(&app, &cookie_a, bd_b_id).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "user A requesting user B's braindump by id must get 404"
    );
    let (status, _body) = get_braindump(&app, &cookie_b, bd_a_id).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "user B requesting user A's braindump by id must get 404"
    );

    // --- /me returns is_admin and display_name ---
    let request = Request::builder()
        .uri("/me")
        .header(COOKIE, &cookie_a)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let me: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["user_id"].as_str().unwrap(), ADMIN_ID);
    assert_eq!(me["display_name"].as_str().unwrap(), "me");
    assert!(me["is_admin"].as_bool().unwrap());

    let request = Request::builder()
        .uri("/me")
        .header(COOKIE, &cookie_b)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let me: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(me["user_id"].as_str().unwrap(), USER_B_ID);
    assert!(!me["is_admin"].as_bool().unwrap());
}

#[tokio::test]
async fn per_user_ontology_isolation() {
    // Each user starts from the day-zero seed and evolves their own vocabulary.
    // A type proposed by user A does not appear in user B's ontology.
    let (app, db) = app_with_extraction(extraction(&["Concept"], &[]));

    // Seed ontology for user B (needs a users row first).
    db.with_conn_test(|conn| {
        conn.execute(
            "INSERT INTO users (id, display_name, is_admin, created_at)
             VALUES (?1, 'user_b', 0, unixepoch())",
            rusqlite::params![USER_B_ID],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    second_brain_backend::db::seed_ontology_for_user(&db, USER_B_ID)
        .await
        .unwrap();

    let cookie_a = session_cookie(&db, ADMIN_ID).await;
    let cookie_b = session_cookie(&db, USER_B_ID).await;

    // User A proposes a new type.
    let request = Request::builder()
        .method("POST")
        .uri("/ontology/propose")
        .header(COOKIE, &cookie_a)
        .header("content-type", "application/json")
        .body(Body::from(
            json!({
                "slug": "nurtures",
                "label": "Nurtures",
                "description": "A nurtures B.",
                "merge_of": null,
            })
            .to_string(),
        ))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let proposal: Value = serde_json::from_slice(&bytes).unwrap();
    let proposal_id = proposal["id"].as_i64().unwrap();

    // User A approves the proposal.
    let request = Request::builder()
        .method("POST")
        .uri(format!("/ontology/proposals/{proposal_id}/approve"))
        .header(COOKIE, &cookie_a)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // User A's ontology includes "nurtures".
    let request = Request::builder()
        .uri("/ontology")
        .header(COOKIE, &cookie_a)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let ont_a: Value = serde_json::from_slice(&bytes).unwrap();
    let slugs_a: Vec<&str> = ont_a["edge_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["slug"].as_str().unwrap())
        .collect();
    assert!(
        slugs_a.contains(&"nurtures"),
        "user A's ontology includes the approved type: {slugs_a:?}"
    );

    // User B's ontology does NOT include "nurtures".
    let request = Request::builder()
        .uri("/ontology")
        .header(COOKIE, &cookie_b)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let ont_b: Value = serde_json::from_slice(&bytes).unwrap();
    let slugs_b: Vec<&str> = ont_b["edge_types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["slug"].as_str().unwrap())
        .collect();
    assert!(
        !slugs_b.contains(&"nurtures"),
        "user B's ontology does NOT include user A's approved type: {slugs_b:?}"
    );
    // User B still has the day-zero vocabulary.
    assert!(
        slugs_b.contains(&"causes"),
        "user B has the day-zero seed: {slugs_b:?}"
    );

    // User A's proposals don't appear in user B's proposal list.
    let request = Request::builder()
        .uri("/ontology/proposals")
        .header(COOKIE, &cookie_b)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let proposals_b: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        proposals_b["proposals"].as_array().unwrap().is_empty(),
        "user B sees no proposals from user A"
    );
}
