//! Admin routes. `/admin/logs` (issue #4) is the hidden admin tab's pull-based
//! log view — the backend reads its own in-memory log ring buffer and surfaces
//! it here so the phone can show errors (e.g. Gemini generation failures)
//! without SSH. `/admin/invites*` (issue #73) lets the admin mint single-use
//! invitations that gate future passkey registration.
//!
//! `/admin/logs` is behind `require_session` — only an authenticated user can
//! read backend internals. `/admin/invites*` is additionally behind
//! `require_admin` (issue #73) — only an admin can mint or list invitations.
//! The buffer is bounded (fixed capacity) and the `?limit` query caps the log
//! response, so the endpoint is VPS-safe on the 8 GB box.

use axum::extract::{Extension, Query, State};
use axum::response::Json;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use crate::auth::session::{random_bearer_token, SessionInfo};
use crate::db::now_seconds;
use crate::error::Result;
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

/// One invitation row, as returned by both mint and list. The `token` is the
/// one-time bearer the admin shares out-of-band with the invitee; it is shown
/// in the list so an admin can re-share a still-pending invite. `status` is
/// `pending` until a registration flow consumes the token, then `consumed`.
/// `consumed_by_display_name` is the invitee's display name (NULL while
/// pending) — a convenience join so the admin list reads as human-readable
/// without a second round-trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invitation {
    pub id: i64,
    pub token: String,
    pub created_by_user_id: String,
    pub status: String,
    pub created_at: i64,
    pub consumed_at: Option<i64>,
    pub consumed_by_user_id: Option<String>,
    pub consumed_by_display_name: Option<String>,
}

/// `POST /admin/invites` — mint a fresh single-use invitation. Admin-only (the
/// `require_admin` guard runs upstream). Returns the new token and metadata;
/// the token is shown once to the admin and shared out-of-band with the
/// invitee, who consumes it in a later slice's registration flow (issue #74).
pub async fn mint_invite(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
) -> Result<Json<Invitation>> {
    let created_by = session.user_id.clone();
    let invitation = state
        .db
        .with_conn(move |conn| mint_invite_conn(conn, &created_by))
        .await?;
    Ok(Json(invitation))
}

fn mint_invite_conn(conn: &rusqlite::Connection, created_by: &str) -> Result<Invitation> {
    let token = random_bearer_token();
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO invitations (token, created_by_user_id, status, created_at)
         VALUES (?1, ?2, 'pending', ?3)",
        params![token, created_by, created_at],
    )?;
    let id = conn.last_insert_rowid();
    Ok(Invitation {
        id,
        token,
        created_by_user_id: created_by.to_string(),
        status: "pending".to_string(),
        created_at,
        consumed_at: None,
        consumed_by_user_id: None,
        consumed_by_display_name: None,
    })
}

/// `GET /admin/invites` — list every invitation (pending and consumed) with
/// consumer info. Admin-only. Ordered newest-first so the freshest mint is at
/// the top of the admin tab. The token is included so a pending invite can be
/// re-shared; the consumed rows retain their token for audit.
pub async fn list_invites(State(state): State<AppState>) -> Result<Json<InvitationsResponse>> {
    let invitations = state.db.with_conn(list_invites_conn).await?;
    Ok(Json(InvitationsResponse { invitations }))
}

#[derive(Serialize)]
pub struct InvitationsResponse {
    pub invitations: Vec<Invitation>,
}

fn list_invites_conn(conn: &rusqlite::Connection) -> Result<Vec<Invitation>> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.token, i.created_by_user_id, i.status, i.created_at,
                i.consumed_at, i.consumed_by_user_id, u.display_name
         FROM invitations i
         LEFT JOIN users u ON u.id = i.consumed_by_user_id
         ORDER BY i.created_at DESC, i.id DESC",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(Invitation {
            id: r.get(0)?,
            token: r.get(1)?,
            created_by_user_id: r.get(2)?,
            status: r.get(3)?,
            created_at: r.get(4)?,
            consumed_at: r.get(5)?,
            consumed_by_user_id: r.get(6)?,
            consumed_by_display_name: r.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
