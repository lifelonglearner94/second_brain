//! HTTP routes. Each domain module owns its router; `router()` stitches them
//! together. Routes are added incrementally as slices land.

use axum::Router;

use crate::state::AppState;

mod health;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", axum::routing::get(health::health))
}
