//! Admin routes. `/admin/logs` (issue #4) is the hidden admin tab's pull-based
//! log view - the backend reads its own in-memory log ring buffer and surfaces
//! it here so the phone can show errors (e.g. Gemini generation failures)
//! without SSH. `/admin/invites*` (issue #73) lets the admin mint single-use
//! invitations that gate future passkey registration. `/admin/system` (#81)
//! surfaces live host load (CPU/RAM/disk) so the operator reads VPS pressure
//! from the phone without SSH.
//!
//! `/admin/logs` and `/admin/system` are behind `require_session` - only an
//! authenticated user can read backend internals. `/admin/invites*` is
//! additionally behind `require_admin` (issue #73) - only an admin can mint or
//! list invitations. The log buffer is bounded (fixed capacity) and the
//! `?limit` query caps the log response, so the endpoint is VPS-safe on the
//! 8 GB box.

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

/// `GET /admin/logs` - return up to `?limit` (default 200, capped at capacity)
/// most-recent structured log entries, newest-first. Newest-first so the admin
/// tab's top-down list shows the freshest state at the top without scrolling
/// past stale history. Auth-gated upstream.
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

/// CPU load, sampled over a short window. `usage_percent` is the mean of the
/// per-core readings (0–100); `cores` is the logical-core count; `per_core`
/// carries each core's 0–100 so the admin can spot a pinned core. CPU usage is
/// a diff between two samples, so the handler refreshes once, waits the
/// crate's minimum update interval, and refreshes again - a single refresh on
/// a fresh `System` returns 0 (no baseline).
#[derive(Serialize)]
pub struct CpuMetrics {
    pub usage_percent: f32,
    pub cores: usize,
    pub per_core: Vec<f32>,
}

/// Memory pressure. `*_bytes` are raw so the frontend formats human-friendly
/// sizes; `usage_percent` is `used / total * 100` (0 when total is 0, e.g. a
/// container that doesn't expose RAM).
#[derive(Serialize)]
pub struct MemoryMetrics {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub usage_percent: f32,
}

/// One mounted filesystem. `used_bytes` is `total - available`. The Brain File
/// (the SQLite database) lives on exactly one of these; `brain_file_mount` on
/// the response identifies which.
#[derive(Serialize)]
pub struct DiskMetrics {
    pub name: String,
    pub mount_point: String,
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub usage_percent: f32,
}

/// `GET /admin/system` - current host load (CPU, RAM, disk) for the admin
/// panel's system tab (#81). Lets the operator read VPS pressure from the
/// phone without SSH. Behind `require_session` - same auth guard as
/// `/admin/logs`. Sampling is stateless (a fresh `sysinfo::System` per
/// request) so the handler stays self-contained and doesn't widen `AppState`;
/// the CPU double-refresh costs one `MINIMUM_CPU_UPDATE_INTERVAL` of latency,
/// an acceptable price for a low-traffic admin read.
pub async fn system(State(state): State<AppState>) -> Json<SystemResponse> {
    let db_path = state.config.database_url.clone();
    let (cpu, memory) = sample_cpu_and_memory().await;
    let (disks, brain_file_mount) = sample_disks(&db_path);
    Json(SystemResponse {
        cpu,
        memory,
        disks,
        brain_file_mount,
    })
}

#[derive(Serialize)]
pub struct SystemResponse {
    pub cpu: CpuMetrics,
    pub memory: MemoryMetrics,
    pub disks: Vec<DiskMetrics>,
    /// Mount point of the filesystem holding the Brain File (the SQLite db at
    /// `config.database_url`), so the frontend can highlight the right disk.
    /// `None` when the db is `:memory:` (tests) or the path can't be resolved
    /// to a known mount.
    pub brain_file_mount: Option<String>,
}

/// Sample CPU and memory from one `sysinfo::System`. CPU usage needs two
/// refreshes bracketing a wait (it's a diff); memory is instantaneous. Using
/// one `System` for both halves the allocation vs. two separate samplers.
async fn sample_cpu_and_memory() -> (CpuMetrics, MemoryMetrics) {
    let mut sys = sysinfo::System::new();
    // First refresh seeds the baseline; the second (after the wait) yields the
    // diff that `cpu_usage()` reports.
    sys.refresh_cpu_usage();
    tokio::time::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL).await;
    sys.refresh_cpu_usage();
    sys.refresh_memory();

    let per_core: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
    let cores = per_core.len();
    let usage_percent = if cores > 0 {
        per_core.iter().sum::<f32>() / cores as f32
    } else {
        0.0
    };
    let cpu = CpuMetrics {
        usage_percent,
        cores,
        per_core,
    };

    let total = sys.total_memory();
    let used = sys.used_memory();
    let usage_percent = if total > 0 {
        (used as f64 / total as f64 * 100.0) as f32
    } else {
        0.0
    };
    let memory = MemoryMetrics {
        total_bytes: total,
        used_bytes: used,
        usage_percent,
    };

    (cpu, memory)
}

/// Sample every mounted filesystem and identify the one holding the Brain File.
/// `Disks` is sampled per request (disk usage is instantaneous - no baseline
/// needed, unlike CPU), so it doesn't need to live in `AppState`.
fn sample_disks(db_path: &str) -> (Vec<DiskMetrics>, Option<String>) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let metrics: Vec<DiskMetrics> = disks
        .iter()
        .map(|d| {
            let total = d.total_space();
            let used = total.saturating_sub(d.available_space());
            let usage_percent = if total > 0 {
                (used as f64 / total as f64 * 100.0) as f32
            } else {
                0.0
            };
            DiskMetrics {
                name: d.name().to_string_lossy().to_string(),
                mount_point: d.mount_point().to_string_lossy().to_string(),
                total_bytes: total,
                used_bytes: used,
                usage_percent,
            }
        })
        .collect();
    let brain_file_mount = brain_file_mount_point(&disks, db_path);
    (metrics, brain_file_mount)
}

/// The mount point of the filesystem holding the Brain File. Resolves the db
/// path to an absolute path and picks the disk whose mount point is the longest
/// prefix of it (longest-prefix handles `/` vs `/home` vs `/data` correctly).
/// Returns `None` for `:memory:` (no file) or if the path can't be canonicalised.
fn brain_file_mount_point(disks: &sysinfo::Disks, db_path: &str) -> Option<String> {
    if db_path.is_empty() || db_path == ":memory:" {
        return None;
    }
    let abs = std::fs::canonicalize(db_path).ok()?;
    let abs_str = abs.to_string_lossy();
    disks
        .iter()
        .filter_map(|d| {
            let mp = d.mount_point().to_string_lossy().to_string();
            if mp.is_empty() {
                None
            } else if abs_str.starts_with(&mp) {
                Some(mp)
            } else {
                None
            }
        })
        .max_by_key(|mp| mp.len())
}

/// One invitation row, as returned by both mint and list. The `token` is the
/// one-time bearer the admin shares out-of-band with the invitee; it is shown
/// in the list so an admin can re-share a still-pending invite. `status` is
/// `pending` until a registration flow consumes the token, then `consumed`.
/// `consumed_by_display_name` is the invitee's display name (NULL while
/// pending) - a convenience join so the admin list reads as human-readable
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

/// `POST /admin/invites` - mint a fresh single-use invitation. Admin-only (the
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

/// `GET /admin/invites` - list every invitation (pending and consumed) with
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
