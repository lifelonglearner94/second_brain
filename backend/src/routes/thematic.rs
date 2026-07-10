//! `GET /thematic` - the Thematic Read Model endpoint (issue #12, ADR-0008).
//!
//! Returns the current Louvain partition of the concept graph with ephemeral
//! "Group N for this session" labels, for the frontend to render. The partition
//! is computed backend-side on every read (the frontend never runs Louvain) and
//! is never persisted - clusters have no stable identity across sessions
//! (ADR-0008). Sits behind the auth middleware (registered in [`crate::routes`]
//! under the protected layer), like the other graph reads.

use axum::extract::{Extension, State};
use axum::response::Json;

use crate::auth::session::SessionInfo;
use crate::error::Result;
use crate::state::AppState;
use crate::thematic::{self, Partition};

/// `GET /thematic` - compute and return the current thematic partition.
pub async fn thematic(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
) -> Result<Json<Partition>> {
    let partition = thematic::partition(&state.db, &session.user_id).await?;
    Ok(Json(partition))
}
