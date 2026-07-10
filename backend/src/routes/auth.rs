//! Auth routes (issue #2): passkey register/login begin/finish, logout, and a
//! stubbed master-passphrase recovery seam. `/me` is the demonstrator of a
//! protected route - gated by [`crate::auth::middleware::require_session`].
//!
//! All flows are JSON-in / JSON-out so a non-browser client (the integration
//! tests use a software passkey) can drive them exactly the way a browser would.
//!
//! Issue #74: registration is invite-gated with a bootstrap exception. The
//! begin handler accepts an optional `{"invite": "<token>"}` body; the finish
//! handler mints a session (registration logs the new user in immediately) and
//! sets the cookie. Login resolves the per-user `user_id` from the authenticated
//! passkey row.

use axum::extract::{Extension, State};
use axum::response::{IntoResponse, Json, Response};
use serde::Serialize;

use crate::auth::cookie::{clear_cookie_response, session_cookie_headers};
use crate::auth::session::{invalidate_session, mint_session, SessionId, SessionInfo};
use crate::auth::webauthn::{
    LoginBegin, LoginFinish, LoginResult, RegistrationBegin, RegistrationBeginRequest,
    RegistrationFinish,
};
use crate::error::Result;
use crate::state::AppState;

/// Begin passkey registration. Issue #74: accepts an optional `{"invite":
/// "<token>"}` body - required once the bootstrap admin exists, ignored while
/// the bootstrap exception is open (zero users). Returns the creation
/// challenge the browser signs plus an opaque `state` token echoed on finish.
pub async fn register_begin(
    State(state): State<AppState>,
    body: Option<Json<RegistrationBeginRequest>>,
) -> Result<Json<RegistrationBegin>> {
    let invite = body.and_then(|b| b.0.invite);
    let begin = state.auth.register_begin(&state.db, invite).await?;
    Ok(Json(begin))
}

/// Finish passkey registration: pair the client's credential with the stored
/// state, consume the invite (or apply the bootstrap exception), create the
/// `users` row, bind the passkey, and mint a session. The session cookie is
/// set so the new user is authenticated immediately. Single-use - the state
/// token is consumed.
pub async fn register_finish(
    State(state): State<AppState>,
    Json(body): Json<RegistrationFinish>,
) -> Result<Response> {
    let session = state.auth.register_finish(&state.db, body).await?;
    let id = SessionId::parse(&session.session_id).expect("minted id is well-formed");
    // The session id rides only in the cookie; the body carries only the
    // account id (JS-readable) so an XSS can't exfiltrate the bearer.
    let body = Json(RegisterOk {
        registered: true,
        user_id: session.user_id,
    });
    Ok((session_cookie_headers(&id), body).into_response())
}

#[derive(Serialize)]
struct RegisterOk {
    registered: bool,
    user_id: String,
}

/// Begin passkey login. Requires at least one registered passkey (across all
/// users - issue #74).
pub async fn login_begin(State(state): State<AppState>) -> Result<Json<LoginBegin>> {
    let begin = state.auth.login_begin(&state.db).await?;
    Ok(Json(begin))
}

/// Finish passkey login: verify the assertion, resolve the per-user `user_id`
/// from the authenticated passkey row (issue #74), mint an opaque session, and
/// set it as an `httpOnly; Secure; SameSite=Strict`, `__Host-`-prefixed cookie.
/// The session row (not the cookie) is the source of truth.
pub async fn login_finish(
    State(state): State<AppState>,
    Json(body): Json<LoginFinish>,
) -> Result<Response> {
    let result: LoginResult = state.auth.login_finish(&state.db, body).await?;
    let session = mint_session(&state.db, &result.user_id).await?;
    let id = SessionId::parse(&session.session_id).expect("minted id is well-formed");
    // The session id rides only in the cookie. Echoing it in a JS-readable body
    // would hand it straight to any XSS - the very thing the `httpOnly` cookie
    // defends against - so the login body carries nothing beyond the account.
    let body = Json(LoginOk {
        user_id: session.user_id,
    });
    Ok((session_cookie_headers(&id), body).into_response())
}

#[derive(Serialize)]
struct LoginOk {
    user_id: String,
}

/// What `/me` hands back. Issue #72: carries `is_admin` and `display_name`
/// (sourced from the `users` table via the session lookup) alongside the
/// account id. Never the session id or expiry, so JS-accessible responses
/// leak no bearer material.
#[derive(Serialize)]
pub struct MeResponse {
    pub user_id: String,
    pub display_name: String,
    pub is_admin: bool,
}

/// Logout: invalidate the session row and clear the cookie. Protected - without
/// a valid cookie there's nothing to log out of.
pub async fn logout(
    State(state): State<AppState>,
    Extension(session): Extension<SessionInfo>,
) -> Result<Response> {
    let id = SessionId::parse(&session.session_id).expect("validated id is well-formed");
    invalidate_session(&state.db, &id).await?;
    Ok(clear_cookie_response(Json(
        serde_json::json!({ "logged_out": true }),
    )))
}

/// `GET /me` - the protected-route demonstrator. Returns the validated session's
/// account id, display name, and admin flag (issue #72). If you can read this,
/// the middleware let you through.
pub async fn me(Extension(session): Extension<SessionInfo>) -> Json<MeResponse> {
    Json(MeResponse {
        user_id: session.user_id,
        display_name: session.display_name,
        is_admin: session.is_admin,
    })
}

/// Master-passphrase recovery seam (issue #2: "stub ok"). The full flow - verify
/// a passphrase, reset credentials, mint a recovery session - is a later slice;
/// this route exists so the client can wire the plumbing and the seam is
/// visible in the router.
pub async fn recover() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "error": "recovery_not_implemented",
        "message": "Master-passphrase recovery is a documented seam; not yet implemented."
    }))
}
