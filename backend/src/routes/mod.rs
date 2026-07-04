//! HTTP routes. Each domain module owns its router; `router()` stitches them
//! together. Routes are added incrementally as slices land.

use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::Router;

use crate::auth::middleware::require_session;
use crate::state::AppState;

mod admin;
mod auth;
mod braindump;
mod health;
mod merge;
mod ontology;

/// Build the full router. State is threaded in here (rather than via
/// `.with_state` on the caller) because the auth middleware needs it at
/// layer-build time — `axum::middleware::from_fn_with_state` is the only
/// `from_fn` variant that lets a middleware extract `State<AppState>`.
pub fn router(state: AppState) -> Router {
    // Public auth routes — no session required. The WebAuthn begin/finish
    // pairs are stateless here; the opaque `state` token carries flow state.
    let auth_routes: Router<AppState> = Router::new()
        .route("/auth/register/begin", post(auth::register_begin))
        .route("/auth/register/finish", post(auth::register_finish))
        .route("/auth/login/begin", post(auth::login_begin))
        .route("/auth/login/finish", post(auth::login_finish))
        .route("/auth/recover", post(auth::recover));

    // Protected routes — every handler behind this layer requires a valid
    // session cookie. `/me` is the demonstrator; `/auth/logout` needs the
    // validated session to invalidate it; `/braindumps` is the ingest write
    // path (ADR-0007) — submit, read, error-correction edit, and deletion with
    // provenance cascade (issue #7); `/merge-suggestions` is the borderline
    // concept-pair queue (ADR-0001 — list, approve, reject); `/admin/logs`
    // surfaces backend logs to the hidden admin tab.
    let protected_routes: Router<AppState> = Router::new()
        .route("/me", get(auth::me))
        .route("/auth/logout", post(auth::logout))
        .route("/braindumps", post(braindump::submit))
        .route(
            "/braindumps/{id}",
            get(braindump::read)
                .patch(braindump::edit)
                .delete(braindump::delete),
        )
        .route("/merge-suggestions", get(merge::list))
        .route("/merge-suggestions/{id}/approve", post(merge::approve))
        .route("/merge-suggestions/{id}/reject", post(merge::reject))
        .route("/admin/logs", get(admin::logs))
        .route_layer(from_fn_with_state(state.clone(), require_session));

    Router::new()
        .route("/health", get(health::health))
        .route("/ontology", get(ontology::ontology))
        .merge(auth_routes)
        .merge(protected_routes)
        .with_state(state)
}
