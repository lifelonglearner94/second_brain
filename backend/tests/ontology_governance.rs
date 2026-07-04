//! Integration tests for issue #9: ontology governance — propose/approve types,
//! type-embedding dedup (>99.5% auto-merge else queued), and the async refactor
//! job that retags existing edges via the append-only type history (ADR-0003).
//!
//! Auth is bypassed by minting a session row directly (as in `braindump.rs`).
//! The embedding client is a scripted stand-in so dedup thresholds land in the
//! auto-merge vs suggestion band deterministically; the LLM is a scripted
//! stand-in returning the re-classification slug so retag is hermetic.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use http::header::COOKIE;
use http::{Request, StatusCode};
use http_body_util::BodyExt;
use second_brain_backend::auth::cookie::request_cookie_header_value;
use second_brain_backend::auth::{mint_session, SessionId};
use second_brain_backend::db::Db;
use second_brain_backend::embedding::EmbeddingClient;
use second_brain_backend::error::Result;
use second_brain_backend::graph;
use second_brain_backend::llm::LlmClient;
use second_brain_backend::routes;
use second_brain_backend::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;

/// The fractal tolerance threshold for ontology types (ADR-0003): stricter
/// than concept identity's 95% because a wrong type merge corrupts every edge
/// using it.
const TYPE_MERGE_THRESHOLD: f32 = 0.995;

async fn session_cookie(db: &Db) -> http::HeaderValue {
    let session = mint_session(db, "00000000-0000-0000-0000-000000000001")
        .await
        .unwrap();
    let id = SessionId::parse(&session.session_id).unwrap();
    request_cookie_header_value(&id)
}

async fn do_request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    cookie: Option<http::HeaderValue>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = &cookie {
        builder = builder.header(COOKIE, c);
    }
    let request = match body {
        Some(b) => {
            builder = builder.header("content-type", "application/json");
            builder.body(Body::from(b.to_string())).unwrap()
        }
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

/// A scripted embedding client: per-text vectors so dedup thresholds land
/// deterministically. `set` a vector for a text; unknown texts fall back to a
/// deterministic non-zero vector (via `deterministic_vector`) so the
/// accretion pipeline's concept KNN never sees a NULL cosine distance on a
/// zero vector.
#[derive(Clone)]
struct ScriptedEmbedding {
    dim: usize,
    vectors: Arc<Mutex<std::collections::HashMap<String, Vec<f32>>>>,
}

impl ScriptedEmbedding {
    fn new(dim: usize) -> Self {
        Self {
            dim,
            vectors: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
    fn set(&self, text: &str, vec: Vec<f32>) {
        self.vectors.lock().unwrap().insert(text.to_string(), vec);
    }
}

#[async_trait]
impl EmbeddingClient for ScriptedEmbedding {
    async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
        if let Some(v) = self.vectors.lock().unwrap().get(text).cloned() {
            return Ok(v);
        }
        // Fall back to a deterministic non-zero vector so concept-embedding KNN
        // (used by accretion) never divides by zero on an all-zeros vector.
        Ok(second_brain_backend::embedding::deterministic_vector(
            text, self.dim,
        ))
    }
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        self.embed_document(text).await
    }
    fn dim(&self) -> usize {
        self.dim
    }
}

/// A scripted LLM: returns the canned slug for every `generate_pinned` call so
/// the refactor's re-classification is hermetic and deterministic. The slug is
/// set per-test via the `set` API; the default is `"causes"` (sanctioned).
#[derive(Clone)]
struct ScriptedLlm {
    slug: Arc<Mutex<String>>,
}

impl ScriptedLlm {
    fn new(slug: &str) -> Self {
        Self {
            slug: Arc::new(Mutex::new(slug.to_string())),
        }
    }
    fn set(&self, slug: &str) {
        *self.slug.lock().unwrap() = slug.to_string();
    }
}

#[async_trait]
impl LlmClient for ScriptedLlm {
    async fn clean(&self, verbatim: &str) -> Result<String> {
        Ok(verbatim.trim().to_string())
    }
    async fn generate_pinned(&self, _system: &str, _user: &str) -> Result<String> {
        Ok(self.slug.lock().unwrap().clone())
    }
}

/// Set scripted embedding vectors for every ontology type's full `type_text`
/// (slug + label + description), using `mapper` to pick a vector per slug.
/// This is what `seed_type_embeddings` will embed when it runs — so the test
/// controls the exact cosine between proposed and existing types.
async fn set_ontology_vectors<F>(emb: &ScriptedEmbedding, db: &Db, mapper: F)
where
    F: Fn(&str) -> Vec<f32>,
{
    let types = second_brain_backend::ontology::ontology_types(db)
        .await
        .unwrap();
    for (slug, label, desc) in types {
        let text = second_brain_backend::ontology::type_text(&slug, &label, &desc);
        emb.set(&text, mapper(&slug));
    }
}

/// Build an AppState wired for governance tests: scripted embedding (for dedup)
/// and scripted LLM (for refactor), with vec0 tables at the scripted dim. The
/// ontology's day-zero types are embedded with the scripted embedding so dedup
/// can match proposals against them (in production `main.rs` does this at
/// startup; tests must do it explicitly after wiring the scripted embedding).
async fn app_with_state(
    db: Db,
    emb: ScriptedEmbedding,
    llm: ScriptedLlm,
) -> (axum::Router, AppState) {
    db.ensure_vec_tables(emb.dim()).unwrap();
    let mut state = AppState::for_tests(db.clone());
    state.embedding = Arc::new(emb.clone());
    state.llm = Arc::new(llm);
    second_brain_backend::ontology::seed_type_embeddings(&state.db, state.embedding.as_ref())
        .await
        .unwrap();
    let app = routes::router(state.clone());
    (app, state)
}

/// Set explicit orthogonal-ish concept-label vectors for the refactor tests so
/// the accretion pipeline creates distinct concepts (the `deterministic_vector`
/// fallback with dim=2 is too collision-prone for concept identity). "Maria",
/// "Q3 launch", and "Q3 review" are the labels used in `seed_edge`.
fn set_refactor_concept_vectors(emb: &ScriptedEmbedding) {
    emb.set("Maria", vec![1.0, 0.0]);
    emb.set("Q3 launch", vec![0.0, 1.0]);
    // cosine([1,0], [1,1]) = cosine([0,1], [1,1]) = 1/sqrt(2) ≈ 0.707 < 0.80
    // (the suggestion floor) → stays a separate concept.
    emb.set("Q3 review", vec![1.0, 1.0]);
}

/// Ingest one braindump + edge through the accretion pipeline so the refactor
/// has an edge to retag. Returns the edge id (looked up by source/target
/// label and original type). Uses a unique verbatim per call so two seed_edge
/// calls don't retract each other's extraction.
async fn seed_edge(
    state: &AppState,
    verbatim: &str,
    source_label: &str,
    type_slug: &str,
    target_label: &str,
) -> i64 {
    use second_brain_backend::braindump::insert_braindump;
    use second_brain_backend::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use second_brain_backend::graph::ingest_extraction;

    let bd = insert_braindump(&state.db, verbatim, verbatim)
        .await
        .unwrap();
    let extraction = ExtractionResult {
        concepts: vec![
            ExtractedConcept {
                label: source_label.to_string(),
            },
            ExtractedConcept {
                label: target_label.to_string(),
            },
        ],
        edges: vec![ExtractedEdge {
            from_label: source_label.to_string(),
            type_slug: type_slug.to_string(),
            to_label: target_label.to_string(),
        }],
    };
    ingest_extraction(
        &state.db,
        state.embedding.as_ref(),
        bd.id,
        verbatim,
        extraction,
    )
    .await
    .unwrap();

    let source = graph::concept_id_for_label(&state.db, source_label)
        .await
        .unwrap()
        .unwrap();
    let target = graph::concept_id_for_label(&state.db, target_label)
        .await
        .unwrap()
        .unwrap();
    graph::find_edge(&state.db, source, type_slug, target)
        .await
        .unwrap()
        .expect("seeded edge exists")
        .id
}

// --- propose + dedup ---

#[tokio::test]
async fn propose_new_type_with_no_near_match_is_queued_pending() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    // The proposed type's embedding is orthogonal to every existing type's
    // embedding → cosine 0 < threshold → queued.
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    // Every seeded ontology type gets the orthogonal vector → 0 similarity.
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    let llm = ScriptedLlm::new("nurtures");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "propose: {body}");
    assert_eq!(body["status"], "pending", "no near match → queued: {body}");
    assert_eq!(body["slug"], "nurtures");
    // Not yet in the ontology.
    let slugs = graph::ontology_slugs(&db).await.unwrap();
    assert!(
        !slugs.iter().any(|s| s == "nurtures"),
        "pending type is not in the ontology yet: {slugs:?}"
    );
}

#[tokio::test]
async fn propose_duplicate_type_above_threshold_auto_merges() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    // The proposed "causes" duplicate lands at cosine 1.0 to the existing
    // "causes" type embedding — well above the 0.995 threshold → auto-merged.
    emb.set(
        "brings_about Brings about A is the reason B happens.",
        vec![1.0, 0.0],
    );
    // Existing types: "causes" shares the vector, others are orthogonal.
    set_ontology_vectors(&emb, &db, |slug| {
        if slug == "causes" {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        }
    })
    .await;
    let llm = ScriptedLlm::new("causes");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "brings_about",
            "label": "Brings about",
            "description": "A is the reason B happens.",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "propose: {body}");
    assert_eq!(
        body["status"], "auto_merged",
        "above-threshold duplicate auto-merges: {body}"
    );
    assert_eq!(body["near_match_slug"], "causes");
    // The new slug was NOT added — it's the same as the existing type.
    let slugs = graph::ontology_slugs(&db).await.unwrap();
    assert!(
        !slugs.iter().any(|s| s == "brings_about"),
        "auto-merged type must not enter the ontology: {slugs:?}"
    );
    assert!(slugs.iter().any(|s| s == "causes"));
}

#[tokio::test]
async fn propose_borderline_duplicate_below_threshold_is_queued() {
    // Cosine 0.99 — above concept identity's 0.95 floor but below the ontology
    // threshold 0.995 → queued for human confirm/reject, NOT auto-merged.
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    // Existing "causes" type gets [1, 0]; others orthogonal.
    set_ontology_vectors(&emb, &db, |slug| {
        if slug == "causes" {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        }
    })
    .await;
    // Proposed "brings_about" at cosine 0.99 to causes.
    let y = (1.0_f32 - 0.99 * 0.99).sqrt();
    emb.set(
        "brings_about Brings about A is the reason B happens.",
        vec![0.99, y],
    );
    let llm = ScriptedLlm::new("causes");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let (status, body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "brings_about",
            "label": "Brings about",
            "description": "A is the reason B happens.",
        })),
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "propose: {body}");
    assert_eq!(body["status"], "pending", "borderline → queued: {body}");
    // The proposal records the near-match for the human reviewer.
    assert_eq!(body["near_match_slug"], "causes");
    assert!(
        (body["near_match_similarity"].as_f64().unwrap() - 0.99).abs() < 1e-5,
        "similarity is the cosine of the near-match: {}",
        body["near_match_similarity"]
    );
    // Sanity: the threshold is the fractal-tolerance value from ADR-0003.
    let _ = TYPE_MERGE_THRESHOLD;
}

// --- list / approve / reject ---

#[tokio::test]
async fn list_proposals_returns_pending_and_auto_merged() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    // "nurtures" at 45° → cosine 0.707 to both [1,0] and [0,1], below the
    // 0.995 threshold → pending (no auto-merge with causes or anything else).
    let diag = (0.5f32).sqrt();
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![diag, diag],
    );
    emb.set(
        "brings_about Brings about A is the reason B happens.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |slug| {
        if slug == "causes" {
            vec![1.0, 0.0]
        } else {
            vec![0.0, 1.0]
        }
    })
    .await;
    let llm = ScriptedLlm::new("causes");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie.clone()),
    )
    .await;
    do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "brings_about",
            "label": "Brings about",
            "description": "A is the reason B happens.",
        })),
        Some(cookie.clone()),
    )
    .await;

    let (status, body) = do_request(&app, "GET", "/ontology/proposals", None, Some(cookie)).await;
    assert_eq!(status, StatusCode::OK, "list: {body}");
    let proposals = body["proposals"].as_array().expect("proposals array");
    assert_eq!(proposals.len(), 2, "{proposals:?}");
    let statuses: Vec<&str> = proposals
        .iter()
        .map(|p| p["status"].as_str().unwrap())
        .collect();
    assert!(
        statuses.contains(&"pending"),
        "nurtures queued: {statuses:?}"
    );
    assert!(
        statuses.contains(&"auto_merged"),
        "brings_about auto-merged: {statuses:?}"
    );
}

#[tokio::test]
async fn approve_pending_type_adds_it_to_ontology_and_stores_embedding() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    let llm = ScriptedLlm::new("nurtures");
    let (app, _state) = app_with_state(db.clone(), emb.clone(), llm).await;
    let cookie = session_cookie(&db).await;

    let (_, propose_body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie.clone()),
    )
    .await;
    let proposal_id = propose_body["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{proposal_id}/approve"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approve: {body}");
    assert_eq!(body["status"], "approved");

    let slugs = graph::ontology_slugs(&db).await.unwrap();
    assert!(
        slugs.iter().any(|s| s == "nurtures"),
        "approved type is in the ontology: {slugs:?}"
    );

    // Future proposals against "nurtures" now dedup against its stored embedding.
    // (Tests that the type-embedding collection was populated on approve.)
    let near = second_brain_backend::ontology::knn_type(
        &db,
        &emb.embed_document("nurtures Nurtures A nurtures or cares for B.")
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert!(near.is_some(), "type-embedding was stored on approve");
    let (slug, _sim) = near.unwrap();
    assert_eq!(slug, "nurtures");
}

#[tokio::test]
async fn reject_pending_type_does_not_add_it() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    let llm = ScriptedLlm::new("nurtures");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let (_, propose_body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie.clone()),
    )
    .await;
    let proposal_id = propose_body["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{proposal_id}/reject"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "reject: {body}");
    assert_eq!(body["status"], "rejected");

    let slugs = graph::ontology_slugs(&db).await.unwrap();
    assert!(
        !slugs.iter().any(|s| s == "nurtures"),
        "rejected type must not enter the ontology: {slugs:?}"
    );
}

#[tokio::test]
async fn approve_already_resolved_proposal_is_409() {
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    let llm = ScriptedLlm::new("nurtures");
    let (app, _state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let (_, propose_body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie.clone()),
    )
    .await;
    let proposal_id = propose_body["id"].as_i64().unwrap();

    let (status, _body) = do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{proposal_id}/reject"),
        None,
        Some(cookie.clone()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Second resolution must fail: the proposal is already resolved.
    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{proposal_id}/approve"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "double-resolve rejected: {body}"
    );
}

#[tokio::test]
async fn governance_routes_require_a_session() {
    let db = Db::open_in_memory().unwrap();
    let emb = ScriptedEmbedding::new(2);
    let llm = ScriptedLlm::new("causes");
    let (app, _state) = app_with_state(db, emb, llm).await;

    let (status, _) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "x",
            "label": "X",
            "description": "x",
        })),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = do_request(&app, "GET", "/ontology/proposals", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// --- async refactor: retag into history ---

#[tokio::test]
async fn approve_merge_refactor_retags_existing_edges_appending_to_history() {
    // Propose "nurtures" as a merge of "helps". On approve, the existing
    // "helps" edge is retagged to "nurtures" by appending to its type history
    // (never overwriting — ADR-0003). The scripted LLM returns "nurtures" as
    // the re-classification.
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    // The "helps" type's embedding is what the accretion pipeline stores for
    // the seeded concept — we set it to a distinct vector so accretion works.
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    set_refactor_concept_vectors(&emb);
    let llm = ScriptedLlm::new("nurtures");
    let (app, state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    // Seed an edge of type "helps" so the refactor has something to retag.
    let edge_id = seed_edge(
        &state,
        "maria helps q3 launch",
        "Maria",
        "helps",
        "Q3 launch",
    )
    .await;
    let before = graph::edge_type_history(&db, edge_id).await.unwrap();
    assert_eq!(
        before.len(),
        1,
        "seeded edge has index 0 = original assertion"
    );
    assert_eq!(before[0].type_slug, "helps");

    // Propose "nurtures" as a merge-of "helps".
    let (_, propose_body) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
            "merge_of": "helps",
        })),
        Some(cookie.clone()),
    )
    .await;
    let proposal_id = propose_body["id"].as_i64().unwrap();

    let (status, body) = do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{proposal_id}/approve"),
        None,
        Some(cookie),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "approve: {body}");

    // The refactor runs out-of-band. Drive it to completion synchronously in
    // the test so we can assert the resulting history. The route kicks it off
    // via tokio::spawn; here we call the public entry point directly.
    second_brain_backend::ontology::await_pending_refactors(&state).await;

    let after = graph::edge_type_history(&db, edge_id).await.unwrap();
    assert_eq!(
        after.len(),
        2,
        "refactor appended exactly one new entry: {after:?}"
    );
    // Index 0 is the original assertion (immutable — ADR-0003).
    assert_eq!(after[0].seq_index, 0);
    assert_eq!(after[0].type_slug, "helps", "original assertion preserved");
    // Index 1 is the refactor's re-classification.
    assert_eq!(after[1].seq_index, 1);
    assert_eq!(after[1].type_slug, "nurtures", "retagged to the new type");
    // The edge's current type is the projection of the last entry.
    let current = second_brain_backend::ontology::current_edge_type(&db, edge_id)
        .await
        .unwrap()
        .expect("edge has a current type");
    assert_eq!(current, "nurtures");
}

#[tokio::test]
async fn second_refactor_over_already_retagged_edge_appends_a_third_entry() {
    // First refactor: helps → nurtures (merge_of=helps).
    // Second refactor: nurtures → sustains (merge_of=nurtures).
    // The edge's history must be [helps, nurtures, sustains] — proving the
    // refactor reads the *projected current* type (last entry), not the
    // original, and appends rather than overwriting.
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    // "sustains" at 45° → cosine 0.707 to "nurtures" [1,0] (stored on approve)
    // and to existing types [0,1], both below 0.995 → pending (not auto-merged
    // with nurtures, so it can be approved and trigger the second refactor).
    let diag = (0.5f32).sqrt();
    emb.set(
        "sustains Sustains A sustains B over time.",
        vec![diag, diag],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    set_refactor_concept_vectors(&emb);
    let llm = ScriptedLlm::new("nurtures");
    let (app, state) = app_with_state(db.clone(), emb, llm.clone()).await;
    let cookie = session_cookie(&db).await;

    let edge_id = seed_edge(
        &state,
        "maria helps q3 launch",
        "Maria",
        "helps",
        "Q3 launch",
    )
    .await;

    // First merge: helps → nurtures.
    let (_, p1) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
            "merge_of": "helps",
        })),
        Some(cookie.clone()),
    )
    .await;
    do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{}/approve", p1["id"].as_i64().unwrap()),
        None,
        Some(cookie.clone()),
    )
    .await;
    second_brain_backend::ontology::await_pending_refactors(&state).await;
    assert_eq!(
        graph::edge_type_history(&db, edge_id).await.unwrap().len(),
        2
    );

    // Second merge: nurtures → sustains. The refactor must find the edge by its
    // *current* type (nurtures, the projection) and append a third entry.
    llm.set("sustains");
    let (_, p2) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "sustains",
            "label": "Sustains",
            "description": "A sustains B over time.",
            "merge_of": "nurtures",
        })),
        Some(cookie.clone()),
    )
    .await;
    do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{}/approve", p2["id"].as_i64().unwrap()),
        None,
        Some(cookie),
    )
    .await;
    second_brain_backend::ontology::await_pending_refactors(&state).await;

    let history = graph::edge_type_history(&db, edge_id).await.unwrap();
    assert_eq!(
        history.len(),
        3,
        "second refactor appended a third entry: {history:?}"
    );
    assert_eq!(
        history[0].type_slug, "helps",
        "original assertion preserved"
    );
    assert_eq!(history[1].type_slug, "nurtures", "first refactor preserved");
    assert_eq!(history[2].type_slug, "sustains", "second refactor appended");
    assert_eq!(
        second_brain_backend::ontology::current_edge_type(&db, edge_id)
            .await
            .unwrap()
            .unwrap(),
        "sustains",
        "current type is the projection of the last entry"
    );
}

#[tokio::test]
async fn refactor_only_retags_edges_of_the_merged_type() {
    // An edge of type "endangers" must NOT be retagged when "helps" is merged
    // into "nurtures" — the refactor targets only edges whose current type is
    // the merge source.
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    set_refactor_concept_vectors(&emb);
    let llm = ScriptedLlm::new("nurtures");
    let (app, state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let helps_edge = seed_edge(
        &state,
        "maria helps q3 launch",
        "Maria",
        "helps",
        "Q3 launch",
    )
    .await;
    let endangers_edge = seed_edge(
        &state,
        "maria endangers q3 review",
        "Maria",
        "endangers",
        "Q3 review",
    )
    .await;

    let (_, p) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
            "merge_of": "helps",
        })),
        Some(cookie.clone()),
    )
    .await;
    do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{}/approve", p["id"].as_i64().unwrap()),
        None,
        Some(cookie),
    )
    .await;
    second_brain_backend::ontology::await_pending_refactors(&state).await;

    // The "helps" edge was retagged; the "endangers" edge was not.
    assert_eq!(
        graph::edge_type_history(&db, helps_edge)
            .await
            .unwrap()
            .len(),
        2,
        "helps edge retagged"
    );
    assert_eq!(
        graph::edge_type_history(&db, endangers_edge)
            .await
            .unwrap()
            .len(),
        1,
        "endangers edge untouched"
    );
}

#[tokio::test]
async fn approve_new_type_with_no_merge_of_does_not_retag_anything() {
    // A pure-new-type approval (no merge_of) adds the type to the ontology but
    // triggers no refactor — there is no source type to retag from.
    let db = Db::open_in_memory().unwrap();
    let dim = 2;
    let emb = ScriptedEmbedding::new(dim);
    emb.set(
        "nurtures Nurtures A nurtures or cares for B.",
        vec![1.0, 0.0],
    );
    set_ontology_vectors(&emb, &db, |_| vec![0.0, 1.0]).await;
    set_refactor_concept_vectors(&emb);
    let llm = ScriptedLlm::new("nurtures");
    let (app, state) = app_with_state(db.clone(), emb, llm).await;
    let cookie = session_cookie(&db).await;

    let edge_id = seed_edge(
        &state,
        "maria helps q3 launch",
        "Maria",
        "helps",
        "Q3 launch",
    )
    .await;

    let (_, p) = do_request(
        &app,
        "POST",
        "/ontology/propose",
        Some(json!({
            "slug": "nurtures",
            "label": "Nurtures",
            "description": "A nurtures or cares for B.",
        })),
        Some(cookie.clone()),
    )
    .await;
    do_request(
        &app,
        "POST",
        &format!("/ontology/proposals/{}/approve", p["id"].as_i64().unwrap()),
        None,
        Some(cookie),
    )
    .await;
    second_brain_backend::ontology::await_pending_refactors(&state).await;

    assert_eq!(
        graph::edge_type_history(&db, edge_id).await.unwrap().len(),
        1,
        "no merge_of → no refactor"
    );
}
