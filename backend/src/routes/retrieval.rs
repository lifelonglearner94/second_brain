//! `POST /retrieve` — the retrieval read path (issue #8, ADR-0004).
//!
//! Seed-then-expand: the query is Gemini-embedded (query task type),
//! concept-embedding KNN seeds the entry concept(s), typed-edge graph traversal
//! expands the neighbourhood, braindumps from the subgraph (plus
//! braindump-embedding backfill) form the context. Unanchored queries fall back
//! to braindump-vector-direct. The full pipeline lives in [`crate::retrieval`];
//! this handler is the thin HTTP seam.
//!
//! Sits behind the auth middleware (registered in [`crate::routes`] under the
//! protected layer), like the ingest write path.

use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::retrieval::{self, RetrievalResult};
use crate::state::AppState;

/// Body for `POST /retrieve`: the query text. Empty queries are rejected — a
/// retrieval with no query is a no-op.
#[derive(Debug, Deserialize)]
pub struct RetrieveRequest {
    pub query: String,
}

/// `POST /retrieve` — run seed-then-expand retrieval (or the no-seed fallback)
/// for the query and return ranked braindumps plus the traversed edge paths.
pub async fn retrieve(
    State(state): State<AppState>,
    Json(body): Json<RetrieveRequest>,
) -> Result<Json<RetrievalResult>> {
    let query = body.query;
    if query.trim().is_empty() {
        return Err(Error::BadRequest("query must be non-empty".into()));
    }
    let result = retrieval::retrieve(&state.db, state.llm.as_ref(), &query).await?;
    Ok(Json(result))
}
