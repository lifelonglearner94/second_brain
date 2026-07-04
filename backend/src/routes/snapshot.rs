//! `GET /graph` — the Global Topology Snapshot endpoint (issue #27, ADR-0008).
//!
//! Returns the full renderable graph topology in one payload — all concepts,
//! all typed edges with their projected current type (ADR-0003), and the
//! current Louvain partition IDs (ADR-0008) — as a gzipped JSON body. The
//! frontend fetches this wholesale on app load and caches it in IndexedDB for
//! offline rendering; the backend owns all graph computation, including the
//! partition IDs (the frontend never runs Louvain). This is the full read;
//! the incremental read is `GET /graph/delta` (issue #28).
//!
//! The payload is always gzipped (single-user scale, but the frontend fetches
//! it all at once on app load). Sits behind the auth middleware (registered in
//! [`crate::routes`] under the protected layer), like the other graph reads.

use std::io::Write;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::error::{Error, Result};
use crate::snapshot;
use crate::state::AppState;

/// `GET /graph` — return the Global Topology Snapshot as a gzipped JSON body.
pub async fn topology_snapshot(State(state): State<AppState>) -> Result<Response> {
    let snapshot = snapshot::topology_snapshot(&state.db).await?;
    let json = serde_json::to_vec(&snapshot)
        .map_err(|e| Error::internal(format!("snapshot encode: {e}")))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&json)
        .map_err(|e| Error::internal(format!("snapshot gzip: {e}")))?;
    let gzipped = encoder
        .finish()
        .map_err(|e| Error::internal(format!("snapshot gzip finish: {e}")))?;
    let mut response = (StatusCode::OK, Body::from(gzipped)).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    Ok(response)
}
