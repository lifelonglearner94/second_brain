//! Braindump ingest routes (issue #5 / #6, ADR-0007).
//!
//! Issue #84: `submit` is fire-and-forget. It validates non-empty, persists the
//! verbatim immediately with an empty cleaned rendering (a placeholder), and
//! returns the braindump row right away — the HTTP request completes in
//! milliseconds, independent of Gemini availability. The clean → extract →
//! accrete pipeline runs in a background task ([`IngestRunner`]) that later
//! updates the cleaned rendering and accretes the graph; the response still
//! carries the braindump id + created_at so the UI can confirm the submit
//! landed (graph reconciliation is via Delta Sync on next focus). `edit` stays
//! synchronous (ADR-0007 error-correction, rare and bounded): clean →
//! overwrite-in-place → ontology → extract → accrete, with the stale
//! extraction retracted first.
//!
//! `GET /braindumps/:id` returns both renderings; `PATCH /braindumps/:id` is
//! error-correction only (overwrites in place, re-cleans, re-extracts,
//! re-runs accretion with the stale extraction retracted first — ADR-0007; id
//! and created_at untouched); `DELETE /braindumps/:id` cascades through the
//! graph.
//!
//! All sit behind the auth middleware (registered in [`crate::routes`] under
//! the protected layer). The cleaner, extractor, and embedder are all methods
//! on the single [`crate::llm::Llm`] seam; the accretion pipeline is
//! [`crate::graph::ingest_extraction`].

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::auth::session::SessionInfo;
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

/// `POST /braindumps` — submit a braindump (issue #84: fire-and-forget).
/// Validate non-empty, persist the verbatim immediately with an empty cleaned
/// rendering, hand the braindump id to the background ingest runner, and
/// return the stored row right away. No LLM call is on the request path, so
/// the response lands in milliseconds regardless of Gemini availability; the
/// clean → extract → accrete pipeline commits out-of-band and the cleaned
/// rendering + concepts + edges populate once it completes.
pub async fn submit(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let braindump = braindump::submit_braindump(&state.db, &session.user_id, &verbatim).await?;
    tracing::info!(
        braindump_id = braindump.id,
        "braindump submitted: background ingest spawned"
    );
    state
        .ingest_runner
        .spawn(
            state.db.clone(),
            state.llm.clone(),
            state.config.clone(),
            session.user_id.clone(),
            braindump.id,
        )
        .await;
    Ok(Json(braindump))
}

/// `GET /braindumps/:id` — return both the verbatim and the cleaned rendering.
pub async fn read(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    Path(id): Path<i64>,
) -> Result<Json<Braindump>> {
    let Some(braindump) = get_braindump(&state.db, &session.user_id, id).await? else {
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
    Extension(session): Extension<SessionInfo>,
    Path(id): Path<i64>,
    Json(body): Json<BraindumpRequest>,
) -> Result<Json<Braindump>> {
    let verbatim = body.verbatim;
    if verbatim.trim().is_empty() {
        return Err(Error::BadRequest("verbatim must be non-empty".into()));
    }
    let Some((braindump, outcome)) = braindump::ingest_edit(
        &state.db,
        &session.user_id,
        state.llm.as_ref(),
        id,
        &verbatim,
    )
    .await?
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
pub async fn delete(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let deleted = graph::delete_braindump(&state.db, &session.user_id, id).await?;
    if !deleted {
        return Err(Error::NotFound(format!("braindump {id} not found")));
    }
    tracing::debug!(braindump_id = id, "braindump deleted: provenance cascaded");
    Ok(StatusCode::NO_CONTENT)
}
