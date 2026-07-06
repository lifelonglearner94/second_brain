//! Concept merge-suggestion queue routes (issue #7, ADR-0001 / ADR-0010).
//!
//! `GET /merge-suggestions` lists the borderline concept pairs the extractor
//! surfaced. `POST /merge-suggestions/:id/approve` folds the `new` concept into
//! the `existing` one — unioning extraction provenance and repointing edges
//! (ADR-0002 consequence: merges may surface contradictory edges, which
//! provenance makes visible rather than silently resolving). `POST
//! /merge-suggestions/:id/reject` keeps the concepts separate and drops the
//! suggestion. All three sit behind the auth middleware.

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::Json;

use crate::auth::session::SessionInfo;
use crate::error::Result;
use crate::graph::{self, MergeSuggestion};
use crate::state::AppState;

/// `GET /merge-suggestions` — the pending borderline concept pairs (ADR-0001).
pub async fn list(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
) -> Result<Json<Vec<MergeSuggestion>>> {
    let suggestions = state.graph_repo.merge_suggestions(&session.user_id).await?;
    Ok(Json(suggestions))
}

/// `POST /merge-suggestions/:id/approve` — merge the two concepts: union
/// extraction provenance, fold edges onto the surviving node, drop the fold
/// concept and the suggestion.
pub async fn approve(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    graph::approve_merge_suggestion(&state.db, &session.user_id, id).await?;
    tracing::debug!(suggestion_id = id, "merge suggestion approved");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /merge-suggestions/:id/reject` — keep the concepts separate and drop
/// the suggestion.
pub async fn reject(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    graph::reject_merge_suggestion(&state.db, &session.user_id, id).await?;
    tracing::debug!(suggestion_id = id, "merge suggestion rejected");
    Ok(StatusCode::NO_CONTENT)
}
