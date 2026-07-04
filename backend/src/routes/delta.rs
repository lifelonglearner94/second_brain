//! Delta-sync route (issue #28): `GET /graph/delta?since=<ts>` — the
//! incremental read surface backing the frontend's pull-on-focus
//! reconciliation. Returns additions + deletions + retags since the client's
//! cursor, plus a fresh cursor for the next pull.
//!
//! Stateless and pull-only: no WebSocket, no SSE, no server-held session. The
//! timestamp is the client's cursor. Sits behind the auth middleware (registered
//! in [`crate::routes`] under the protected layer), like the other graph reads.

use axum::extract::{Query, State};
use axum::response::Json;
use serde::Deserialize;

use crate::delta::{self, GraphDelta};
use crate::error::Result;
use crate::state::AppState;

/// Query params for `GET /graph/delta`. `since` is the client's last cursor;
/// omitted on a first sync (defaults to 0 → everything is an addition).
#[derive(Debug, Deserialize)]
pub struct DeltaQuery {
    pub since: Option<i64>,
}

/// `GET /graph/delta?since=<ts>` — return the graph changes since the cursor.
pub async fn graph_delta(
    State(state): State<AppState>,
    Query(query): Query<DeltaQuery>,
) -> Result<Json<GraphDelta>> {
    let since = query.since.unwrap_or(0);
    let delta = delta::graph_delta(&state.db, since).await?;
    Ok(Json(delta))
}
