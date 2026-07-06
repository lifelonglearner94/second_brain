//! HTTP routes. Each domain module owns its router; `router()` stitches them
//! together. Routes are added incrementally as slices land.

use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{get, post};
use axum::Router;

use crate::auth::middleware::{require_admin, require_session};
use crate::state::AppState;

mod admin;
mod auth;
mod braindump;
mod chat;
mod chat_inference;
mod delta;
mod health;
mod merge;
mod ontology;
mod retrieval;
mod snapshot;
mod thematic;

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
    // inference, silence when unsupported); `/chat/inferences*` is the
    // governed write-back surface (ADR-0006 — structural + thematic inference
    // proposals enter the queue pending, never auto-endorsed; endorse persists
    // the edge with `structural_inference` or `thematic_inference` provenance;
    // thematic mode (issue #13, ADR-0009) carries a frozen Thematic Snapshot); `/thematic` is the Thematic Read
    // Model endpoint (ADR-0008 — backend-owned Louvain partition with ephemeral
    // session labels, layered into chat as macrostructure context);
    // `/graph` is the Global Topology Snapshot — the full renderable graph in
    // one gzipped payload (all concepts + typed edges with projected current
    // type + current Louvain partition IDs, ADR-0003/0008) the frontend fetches
    // wholesale on app load (issue #27); `/graph/delta` is the incremental read
    // surface for pull-on-focus reconciliation (issue #28); `/admin/logs`
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
        .route(
            "/chat/inferences",
            post(chat_inference::propose).get(chat_inference::list),
        )
        .route(
            "/chat/inferences/thematic",
            post(chat_inference::propose_thematic),
        )
        .route(
            "/chat/inferences/{id}/endorse",
            post(chat_inference::endorse),
        )
        .route("/chat/inferences/{id}/reject", post(chat_inference::reject))
        .route("/thematic", get(thematic::thematic))
        .route("/graph", get(snapshot::topology_snapshot))
        .route("/graph/delta", get(delta::graph_delta))
        .route("/admin/logs", get(admin::logs))
        .route("/ontology", get(ontology::ontology))
        .route("/ontology/propose", post(ontology::propose))
        .route("/ontology/proposals", get(ontology::proposals))
        .route("/ontology/proposals/{id}/approve", post(ontology::approve))
        .route("/ontology/proposals/{id}/reject", post(ontology::reject))
        .route_layer(from_fn_with_state(state.clone(), require_session));

    // Admin-only routes (issue #73) — behind `require_session` (outer, resolves
    // the session and stashes `SessionInfo` in extensions) AND `require_admin`
    // (inner, refuses non-admins with 403 by reading `SessionInfo.is_admin`).
    // The layer order is load-bearing: `require_session` must run first so the
    // extension is present when `require_admin` reads it; chained
    // `.route_layer` calls apply outermost-last, so `require_session` is added
    // second (outer) and `require_admin` first (inner). `/admin/invites` mints
    // and lists single-use invitations that gate future passkey registration.
    let admin_routes: Router<AppState> = Router::new()
        .route(
            "/admin/invites",
            post(admin::mint_invite).get(admin::list_invites),
        )
        .route_layer(from_fn(require_admin))
        .route_layer(from_fn_with_state(state.clone(), require_session));

    Router::new()
        .route("/health", get(health::health))
        .merge(auth_routes)
        .merge(protected_routes)
        .merge(admin_routes)
        .with_state(state)
}
