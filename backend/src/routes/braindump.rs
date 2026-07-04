//! Braindump ingest routes (issue #5 / #6, ADR-0007).
//!
//! The write path, end-to-end through the DB. `POST /braindumps` cleans the
//! verbatim via Gemini, persists verbatim + cleaned + timestamp immutably, then
//! runs the extractor (which draws edge types from the ontology) and the
//! atomic accretion pipeline — identity + provenance + type history + embeddings
//! (ADR-0001 / ADR-0002 / ADR-0003 / ADR-0010). `GET /braindumps/:id` returns
//! both renderings. `PATCH /braindumps/:id` is error-correction only: it
//! overwrites the verbatim in place, re-cleans, re-extracts, and re-runs
//! accretion (the stale extraction is retracted first — ADR-0007); the id and
//! created_at are untouched.
//!
//! All three sit behind the auth middleware (registered in [`crate::routes`]
//! under the protected layer). The cleaner is [`crate::llm::Llm::clean`];
//! extraction is a method on the same [`crate::llm::Llm`] seam; the accretion
//! pipeline is [`crate::graph::ingest_extraction`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::braindump::{get_braindump, insert_braindump, overwrite_verbatim, Braindump};
use crate::error::{Error, Result};
use crate::graph;
use crate::state::AppState;

/// Body for `POST /braindumps` and `PATCH /braindumps/:id`: the user-confirmed
/// verbatim text. Empty verbatim is rejected — a braindump with no text is a
/// capture artefact, not a thought-snapshot.
#[derive(Debug, Deserialize)]
pub struct BraindumpRequest {
    pub verbatim: String,
}

/// `POST /braindumps` — submit a braindump. Cleans the verbatim via the LLM
/// seam, persists verbatim + cleaned + timestamp immutably, then runs extraction
/// + atomic accretion (ADR-0001 / ADR-0002 / ADR-0003 / ADR-0010).
pub async fn submit(
    State(state): State<AppState>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let cleaned = state.llm.clean(&verbatim).await?;
    let braindump = insert_braindump(&state.db, &verbatim, &cleaned).await?;
    let ontology = graph::ontology_slugs(&state.db).await?;
    let extraction = state
        .llm
        .extract(&braindump.verbatim, &ontology)
        .await?;
    let outcome = graph::ingest_extraction(
        &state.db,
        state.llm.as_ref(),
        braindump.id,
        &braindump.verbatim,
        extraction,
    )
    .await?;
    tracing::debug!(
        braindump_id = braindump.id,
        created = outcome.concepts_created,
        accreted = outcome.concepts_accreted,
        suggestions = outcome.merge_suggestions,
        edges_created = outcome.edges_created,
        edges_accreted = outcome.edges_accreted,
        edges_rejected = outcome.edges_rejected,
        "ingest: extraction + accretion complete"
    );
    Ok(Json(braindump))
}

/// `GET /braindumps/:id` — return both the verbatim and the cleaned rendering.
pub async fn read(State(state): State<AppState>, Path(id): Path<i64>) -> Result<Json<Braindump>> {
    let Some(braindump) = get_braindump(&state.db, id).await? else {
        return Err(Error::NotFound(format!("braindump {id} not found")));
    };
    Ok(Json(braindump))
}

/// `PATCH /braindumps/:id` — error-correction only (ADR-0007). Overwrites the
/// verbatim in place, re-runs the cleaner on the corrected text, re-extracts,
/// and re-runs accretion (the stale extraction is retracted first). The id and
/// created_at are untouched; substantive thinking-evolution spawns a new
/// braindump, never edits the old one.
pub async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let cleaned = state.llm.clean(&verbatim).await?;
    let Some(braindump) = overwrite_verbatim(&state.db, id, &verbatim, &cleaned).await? else {
        return Err(Error::NotFound(format!("braindump {id} not found")));
    };
    let ontology = graph::ontology_slugs(&state.db).await?;
    let extraction = state
        .llm
        .extract(&braindump.verbatim, &ontology)
        .await?;
    let outcome = graph::ingest_extraction(
        &state.db,
        state.llm.as_ref(),
        braindump.id,
        &braindump.verbatim,
        extraction,
    )
    .await?;
    tracing::debug!(
        braindump_id = braindump.id,
        created = outcome.concepts_created,
        accreted = outcome.concepts_accreted,
        suggestions = outcome.merge_suggestions,
        edges_created = outcome.edges_created,
        edges_accreted = outcome.edges_accreted,
        edges_rejected = outcome.edges_rejected,
        "edit: re-extraction + accretion complete"
    );
    Ok(Json(braindump))
}

/// `DELETE /braindumps/:id` — remove a braindump and cascade through the graph
/// (ADR-0002 / ADR-0007 / ADR-0010). The braindump's id drops from every
/// concept's extraction provenance and every edge's `asserted_by`; a concept
/// vanishes when its last extracting braindump is removed, an edge vanishes
/// when its last asserter is removed, and an edge whose endpoint concept
/// vanishes is cascade-deleted (ADR-0010 addendum). `404` if no such braindump.
pub async fn delete(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode> {
    let deleted = graph::delete_braindump(&state.db, id).await?;
    if !deleted {
        return Err(Error::NotFound(format!("braindump {id} not found")));
    }
    tracing::debug!(braindump_id = id, "braindump deleted: provenance cascaded");
    Ok(StatusCode::NO_CONTENT)
}
