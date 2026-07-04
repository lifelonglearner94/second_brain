//! Braindump ingest routes (issue #5 / #6, ADR-0007).
//!
//! Thin HTTP adapters (issue #42): `submit` and `edit` parse + validate
//! non-empty, then delegate the full ingest pipeline — clean → persist →
//! ontology → extract → accrete (identity + provenance + type history +
//! embeddings, ADR-0001/0002/0003/0010) — to [`crate::braindump::ingest`] /
//! [`crate::braindump::ingest_edit`], log the outcome, and return the stored
//! braindump as JSON. The pipeline (the spec) lives in `braindump`, not here,
//! so it is unit-testable without an HTTP roundtrip. `GET /braindumps/:id`
//! returns both renderings; `PATCH /braindumps/:id` is error-correction only
//! (overwrites in place, re-cleans, re-extracts, re-runs accretion with the
//! stale extraction retracted first — ADR-0007; id and created_at untouched);
//! `DELETE /braindumps/:id` cascades through the graph.
//!
//! All sit behind the auth middleware (registered in [`crate::routes`] under
//! the protected layer). The cleaner, extractor, and embedder are all methods
//! on the single [`crate::llm::Llm`] seam; the accretion pipeline is
//! [`crate::graph::ingest_extraction`].

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::braindump::{self, get_braindump, Braindump};
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

/// `POST /braindumps` — submit a braindump. A thin HTTP adapter (issue #42):
/// parse + validate non-empty, delegate the full ingest pipeline
/// (clean → persist → ontology → extract → accrete, ADR-0007) to
/// [`braindump::ingest`], log the outcome, and return the stored braindump.
pub async fn submit(
    State(state): State<AppState>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let (braindump, outcome) = braindump::ingest(&state.db, state.llm.as_ref(), &verbatim).await?;
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

/// `PATCH /braindumps/:id` — error-correction only (ADR-0007). A thin HTTP
/// adapter (issue #42): parse + validate non-empty, delegate the full
/// re-ingest pipeline (clean → overwrite-in-place → ontology → extract →
/// accrete, with the stale extraction retracted first) to
/// [`braindump::ingest_edit`], log the outcome, and return the braindump.
/// `404` if no braindump with `id` exists. The id and created_at are
/// untouched; substantive thinking-evolution spawns a new braindump, never
/// edits the old one.
pub async fn edit(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let Some((braindump, outcome)) =
        braindump::ingest_edit(&state.db, state.llm.as_ref(), id, &verbatim).await?
    else {
        return Err(Error::NotFound(format!("braindump {id} not found")));
    };
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
