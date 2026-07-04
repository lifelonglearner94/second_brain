//! Integration tests for issue #28: the delta-sync endpoint
//! `GET /graph/delta?since=<ts>` — additions + deletions + retags for the
//! frontend's pull-on-focus reconciliation.
//!
//! Concepts/edges are created via the real submit→extract→accrete path; auth is
//! bypassed by minting a session row directly (as in `deletion.rs`). Retags are
//! simulated by appending to `edge_type_history` directly (the refactor pipeline
//! is exercised end-to-end in `ontology_governance.rs`; here we test the delta
//! read surface, not the refactor).

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
        Ok("ScriptedLlm::synthesize (unused by delta-sync tests)".to_string())
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
        Ok("SequencedLlm::synthesize (unused by delta-sync tests)".to_string())
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

async fn delete(app: &axum::Router, cookie: &http::HeaderValue, id: i64) -> StatusCode {
    let request = Request::builder()
        .method("DELETE")
        .uri(format!("/braindumps/{id}"))
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    response.status()
}

async fn delta(
    app: &axum::Router,
    cookie: &http::HeaderValue,
    since: Option<i64>,
) -> (StatusCode, Value) {
    let uri = match since {
        Some(ts) => format!("/graph/delta?since={ts}"),
        None => "/graph/delta".to_string(),
    };
    let request = Request::builder()
        .method("GET")
        .uri(uri)
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

/// Stamp every graph row's `created_at` to a fixed value so delta-filtering on
/// a cursor between the stamp and a later retag is deterministic.
async fn backdate_graph(db: &Db, ts: i64) {
    db.run(move |conn| {
        conn.execute("UPDATE concepts SET created_at = ?1", rusqlite::params![ts])?;
        conn.execute("UPDATE edges SET created_at = ?1", rusqlite::params![ts])?;
        conn.execute(
            "UPDATE edge_type_history SET created_at = ?1",
            rusqlite::params![ts],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

/// Append a retag entry (seq_index = max+1) to an edge's type history —
/// simulates the async refactor (ADR-0003) without standing up governance.
async fn append_retag(db: &Db, edge_id: i64, type_slug: &str) {
    let type_slug = type_slug.to_string();
    db.run(move |conn| {
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

#[tokio::test]
async fn delta_returns_additions_after_ingest() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    submit(&app, &cookie, "maria endangers q3 launch").await;

    let (status, body) = delta(&app, &cookie, Some(0)).await;
    assert_eq!(status, StatusCode::OK, "delta: {body}");
    let added = body["added_concepts"]
        .as_array()
        .expect("added_concepts array");
    assert_eq!(added.len(), 2, "both concepts added: {added:?}");
    assert!(
        added.iter().any(|c| c["label"] == "Maria"),
        "Maria in additions"
    );
    let edges = body["added_edges"].as_array().expect("added_edges array");
    assert_eq!(edges.len(), 1, "one edge added: {edges:?}");
    assert_eq!(edges[0]["original_type"], "endangers");
    assert_eq!(edges[0]["current_type"], "endangers");
    assert_eq!(body["deleted_concept_ids"].as_array().unwrap().len(), 0);
    assert_eq!(body["deleted_edge_ids"].as_array().unwrap().len(), 0);
    assert_eq!(body["retagged_edges"].as_array().unwrap().len(), 0);
    assert!(body["cursor"].as_i64().unwrap() > 0, "cursor returned");
}

#[tokio::test]
async fn delta_returns_deletions_after_braindump_delete() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "endangers", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd = submit(&app, &cookie, "maria endangers q3 launch").await;
    let maria = graph::concept_id_for_label(&db, "Maria")
        .await
        .unwrap()
        .unwrap();
    let q3 = graph::concept_id_for_label(&db, "Q3 launch")
        .await
        .unwrap()
        .unwrap();
    let edge_row = graph::find_edge(&db, maria, "endangers", q3)
        .await
        .unwrap()
        .expect("edge exists");

    let status = delete(&app, &cookie, bd).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = delta(&app, &cookie, Some(0)).await;
    assert_eq!(status, StatusCode::OK, "delta: {body}");
    let deleted_concepts = body["deleted_concept_ids"]
        .as_array()
        .expect("deleted_concept_ids array");
    assert!(
        deleted_concepts.iter().any(|c| c.as_i64() == Some(maria)),
        "Maria in deletions: {deleted_concepts:?}"
    );
    assert!(
        deleted_concepts.iter().any(|c| c.as_i64() == Some(q3)),
        "Q3 launch in deletions: {deleted_concepts:?}"
    );
    let deleted_edges = body["deleted_edge_ids"]
        .as_array()
        .expect("deleted_edge_ids array");
    assert!(
        deleted_edges
            .iter()
            .any(|e| e.as_i64() == Some(edge_row.id)),
        "edge in deletions: {deleted_edges:?}"
    );
}

#[tokio::test]
async fn delta_returns_retags_after_edge_type_refactor() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria", "Q3 launch"]),
            edges: vec![edge("Maria", "helps", "Q3 launch")],
        },
    });
    let app = app_with_llm(db.clone(), llm);
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

    // Backdate the graph to before the cursor, then retag after it.
    backdate_graph(&db, 1000).await;
    append_retag(&db, edge_row.id, "supports").await;

    let (status, body) = delta(&app, &cookie, Some(1500)).await;
    assert_eq!(status, StatusCode::OK, "delta: {body}");
    assert_eq!(
        body["added_concepts"].as_array().unwrap().len(),
        0,
        "nothing created after cursor"
    );
    assert_eq!(
        body["added_edges"].as_array().unwrap().len(),
        0,
        "no edge created after cursor"
    );
    let retags = body["retagged_edges"].as_array().expect("retagged_edges");
    assert_eq!(retags.len(), 1, "one retag: {retags:?}");
    assert_eq!(retags[0]["id"], edge_row.id);
    assert_eq!(retags[0]["original_type"], "helps");
    assert_eq!(retags[0]["current_type"], "supports");
    assert_eq!(retags[0]["source_concept_id"], maria);
    assert_eq!(retags[0]["target_concept_id"], q3);
}

#[tokio::test]
async fn delta_demo_returns_all_three_change_types() {
    // The issue's acceptance demo: ingest → delete a braindump → approve a
    // retag → delta returns additions + deletions + retags in one response.
    //
    // bd1 seeds the edge to retag (backdated before the cursor so the retag is
    // not redundant with its addition); bd2 seeds entities that get deleted;
    // bd3 seeds fresh additions. Controlled timestamps keep the boundary
    // deterministic.
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(SequencedLlm {
        calls: Mutex::new(0),
        results: vec![
            // bd1: Maria —[helps]→ Q3 launch (will be retagged, survives).
            ExtractionResult {
                concepts: concepts(&["Maria", "Q3 launch"]),
                edges: vec![edge("Maria", "helps", "Q3 launch")],
            },
            // bd2: Beta —[endangers]→ Risk (will be deleted via bd2 deletion).
            ExtractionResult {
                concepts: concepts(&["Beta", "Risk"]),
                edges: vec![edge("Beta", "endangers", "Risk")],
            },
            // bd3: Calm —[causes]→ Focus (fresh addition, survives).
            ExtractionResult {
                concepts: concepts(&["Calm", "Focus"]),
                edges: vec![edge("Calm", "causes", "Focus")],
            },
        ],
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    let bd1 = submit(&app, &cookie, "maria helps q3 launch").await;
    let bd2 = submit(&app, &cookie, "beta endangers risk").await;
    let helps_edge = graph::find_edge(
        &db,
        graph::concept_id_for_label(&db, "Maria")
            .await
            .unwrap()
            .unwrap(),
        "helps",
        graph::concept_id_for_label(&db, "Q3 launch")
            .await
            .unwrap()
            .unwrap(),
    )
    .await
    .unwrap()
    .expect("helps edge");
    let beta = graph::concept_id_for_label(&db, "Beta")
        .await
        .unwrap()
        .unwrap();
    let risk = graph::concept_id_for_label(&db, "Risk")
        .await
        .unwrap()
        .unwrap();
    let endangers_edge = graph::find_edge(&db, beta, "endangers", risk)
        .await
        .unwrap()
        .expect("endangers edge");

    // Backdate bd1's graph to before the cursor so its retag is not redundant
    // with an addition (the edge was created before the cursor).
    backdate_graph(&db, 1000).await;
    let since = 1500;

    // bd3 is a fresh addition (created after the cursor).
    submit(&app, &cookie, "calm causes focus").await;
    // Retag bd1's edge (refactor after the cursor).
    append_retag(&db, helps_edge.id, "supports").await;
    // Delete bd2 → Beta, Risk, endangers edge vanish (deletions after cursor).
    let status = delete(&app, &cookie, bd2).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, body) = delta(&app, &cookie, Some(since)).await;
    assert_eq!(status, StatusCode::OK, "delta demo: {body}");

    // Additions: Calm, Focus, and the causes edge (from bd3).
    let added_labels: Vec<&str> = body["added_concepts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["label"].as_str().unwrap())
        .collect();
    assert!(
        added_labels.contains(&"Calm"),
        "Calm in additions: {added_labels:?}"
    );
    assert!(
        added_labels.contains(&"Focus"),
        "Focus in additions: {added_labels:?}"
    );
    assert_eq!(
        body["added_edges"].as_array().unwrap().len(),
        1,
        "one edge added (causes)"
    );

    // Deletions: Beta, Risk, endangers edge.
    let deleted_concepts = body["deleted_concept_ids"].as_array().unwrap();
    assert!(
        deleted_concepts.iter().any(|c| c.as_i64() == Some(beta)),
        "Beta in deletions: {deleted_concepts:?}"
    );
    assert!(
        deleted_concepts.iter().any(|c| c.as_i64() == Some(risk)),
        "Risk in deletions: {deleted_concepts:?}"
    );
    let deleted_edges = body["deleted_edge_ids"].as_array().unwrap();
    assert!(
        deleted_edges
            .iter()
            .any(|e| e.as_i64() == Some(endangers_edge.id)),
        "endangers edge in deletions: {deleted_edges:?}"
    );

    // Retags: the helps → supports edge.
    let retags = body["retagged_edges"].as_array().unwrap();
    assert_eq!(retags.len(), 1, "one retag: {retags:?}");
    assert_eq!(retags[0]["id"], helps_edge.id);
    assert_eq!(retags[0]["original_type"], "helps");
    assert_eq!(retags[0]["current_type"], "supports");

    // bd1's concepts (Maria, Q3 launch) are backdated before the cursor and
    // neither deleted nor retagged → absent from the delta entirely.
    assert!(
        !added_labels.contains(&"Maria"),
        "Maria is not an addition (created before cursor)"
    );

    // bd1 was not deleted — confirm it still exists so the retag is meaningful.
    assert!(graph::get_concept(
        &db,
        graph::concept_id_for_label(&db, "Maria")
            .await
            .unwrap()
            .unwrap()
    )
    .await
    .unwrap()
    .is_some());
    // Suppress unused warning for bd1 in the final assertion path.
    let _ = bd1;
}

#[tokio::test]
async fn delta_with_no_since_defaults_to_first_sync() {
    // Omitting ?since defaults to 0 — a first sync returning everything as
    // additions.
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria"]),
            edges: vec![],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    submit(&app, &cookie, "maria").await;

    let (status, body) = delta(&app, &cookie, None).await;
    assert_eq!(status, StatusCode::OK, "delta no-since: {body}");
    assert_eq!(
        body["added_concepts"].as_array().unwrap().len(),
        1,
        "first sync returns the concept as an addition"
    );
    assert!(body["cursor"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn delta_returns_empty_when_nothing_changed_since_cursor() {
    let db = Db::open_in_memory().unwrap();
    let llm = Arc::new(ScriptedLlm {
        result: ExtractionResult {
            concepts: concepts(&["Maria"]),
            edges: vec![],
        },
    });
    let app = app_with_llm(db.clone(), llm);
    let cookie = session_cookie(&db).await;

    submit(&app, &cookie, "maria").await;

    // Use a far-future cursor so nothing is newer.
    let (status, body) = delta(&app, &cookie, Some(i64::MAX - 1)).await;
    assert_eq!(status, StatusCode::OK, "delta: {body}");
    assert_eq!(body["added_concepts"].as_array().unwrap().len(), 0);
    assert_eq!(body["added_edges"].as_array().unwrap().len(), 0);
    assert_eq!(body["deleted_concept_ids"].as_array().unwrap().len(), 0);
    assert_eq!(body["deleted_edge_ids"].as_array().unwrap().len(), 0);
    assert_eq!(body["retagged_edges"].as_array().unwrap().len(), 0);
    assert!(
        body["cursor"].as_i64().unwrap() > 0,
        "cursor is a fresh timestamp"
    );
}

#[tokio::test]
async fn delta_requires_a_session() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db));

    let request = Request::builder()
        .method("GET")
        .uri("/graph/delta?since=0")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "delta without session is rejected"
    );
}

#[tokio::test]
async fn delta_rejects_non_numeric_since() {
    let db = Db::open_in_memory().unwrap();
    let app = routes::router(AppState::for_tests(db.clone()));
    let cookie = session_cookie(&db).await;

    let request = Request::builder()
        .method("GET")
        .uri("/graph/delta?since=not-a-number")
        .header(COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "non-numeric since is rejected"
    );
}
