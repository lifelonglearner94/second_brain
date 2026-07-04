//! Braindump ingest routes (issue #5 / ADR-0007).
//!
//! The write path, end-to-end through the DB, with extraction stubbed:
//! * `POST /braindumps` — verbatim → Gemini clean → persist verbatim + cleaned
//!   + timestamp → run the (stubbed) extractor.
//! * `GET /braindumps/:id` — returns both renderings.
//! * `PATCH /braindumps/:id` — error-correction only: overwrites the verbatim
//!   in place, re-cleans, and re-runs the (stubbed) extractor. The id and
//!   created_at are untouched.
//!
//! All three sit behind the auth middleware (registered in [`crate::routes`]
//! under the protected layer). The cleaner is [`crate::llm::LlmClient::clean`];
//! the extractor is [`crate::extractor::Extractor`].

use axum::extract::{Path, State};
use axum::response::Json;
use serde::Deserialize;

use crate::braindump::{get_braindump, insert_braindump, overwrite_verbatim, Braindump};
use crate::error::{Error, Result};
use crate::state::AppState;

/// Body for `POST /braindumps` and `PATCH /braindumps/:id`: the user-confirmed
/// verbatim text. Empty verbatim is rejected — a braindump with no text is a
/// capture artefact, not a thought-snapshot.
#[derive(Debug, Deserialize)]
pub struct BraindumpRequest {
    pub verbatim: String,
}

/// `POST /braindumps` — submit a braindump. Cleans the verbatim via the LLM
/// seam, persists verbatim + cleaned + timestamp immutably, and runs the
/// extractor seam (stubbed: returns no concepts/edges).
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
    let extraction = state.extractor.extract(&braindump.verbatim).await?;
    tracing::debug!(
        braindump_id = braindump.id,
        concepts = extraction.concepts.len(),
        edges = extraction.edges.len(),
        "ingest: extraction complete (stub returns none in this slice)"
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
/// verbatim in place, re-runs the cleaner on the corrected text, and re-runs
/// the (stubbed) extractor. The id and created_at are untouched; substantive
/// thinking-evolution spawns a new braindump, never edits the old one.
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
    let extraction = state.extractor.extract(&braindump.verbatim).await?;
    tracing::debug!(
        braindump_id = braindump.id,
        concepts = extraction.concepts.len(),
        edges = extraction.edges.len(),
        "edit: re-extraction complete (stub returns none in this slice)"
    );
    Ok(Json(braindump))
}
