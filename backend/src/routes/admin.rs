//! Admin routes (issue #4): the hidden admin tab's pull-based log view. The
//! backend reads its own in-memory log ring buffer and surfaces it here so the
//! phone can show errors (e.g. Gemini generation failures) without SSH.
//!
//! `/admin/logs` is behind `require_session` — only the authenticated owner
//! can read backend internals. The buffer is bounded (fixed capacity) and the
//! `?limit` query caps the response, so the endpoint is VPS-safe on the 8 GB
//! single-user box.

use axum::extract::{Query, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::logs::LogEntry;
use crate::state::AppState;

/// Default page size when the client omits `?limit`. Capped under the buffer's
/// capacity so a default fetch never ships the whole ring across the wire.
const DEFAULT_LIMIT: usize = 200;

#[derive(Deserialize)]
pub struct LogsQuery {
    pub limit: Option<usize>,
}

#[derive(Serialize)]
pub struct LogsResponse {
    logs: Vec<LogEntry>,
    count: usize,
    capacity: usize,
}

/// `GET /admin/logs` — return up to `?limit` (default 200, capped at capacity)
/// most-recent structured log entries, oldest-first. Auth-gated upstream.
pub async fn logs(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Json<LogsResponse> {
    let capacity = state.log_buffer.capacity();
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).min(capacity);
    let entries = state.log_buffer.recent(limit);
    let count = entries.len();
    Json(LogsResponse {
        logs: entries,
        count,
        capacity,
    })
}
