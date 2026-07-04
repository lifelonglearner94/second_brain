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
mod chat;
mod delta;
mod health;
mod merge;
mod ontology;
mod retrieval;

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
    // concept-pair queue (ADR-0001 — list, approve, reject); `/retrieve` is
    // the seed-then-expand read path (ADR-0004); `/chat` is the grounded-
    // synthesis read surface (ADR-0005 — mandatory citations, graph-constrained
    // inference, silence when unsupported); `/graph/delta` is the incremental
    // read surface for pull-on-focus reconciliation (issue #28); `/admin/logs`
    // surfaces backend logs to the hidden admin tab; `/ontology/propose*` is
    // the governance queue (ADR-0003) — propose/approve/reject edge types and
    // trigger the async refactor.
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
        .route("/retrieve", post(retrieval::retrieve))
        .route("/chat", post(chat::chat))
        .route("/graph/delta", get(delta::graph_delta))
        .route("/admin/logs", get(admin::logs))
        .route("/ontology/propose", post(ontology::propose))
        .route("/ontology/proposals", get(ontology::proposals))
        .route("/ontology/proposals/{id}/approve", post(ontology::approve))
        .route("/ontology/proposals/{id}/reject", post(ontology::reject))
        .route_layer(from_fn_with_state(state.clone(), require_session));

    Router::new()
        .route("/health", get(health::health))
        .route("/ontology", get(ontology::ontology))
        .merge(auth_routes)
        .merge(protected_routes)
        .with_state(state)
}
