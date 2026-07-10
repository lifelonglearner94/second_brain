//! Auth middleware: gates every non-auth route behind a valid session cookie.
//!
//! Reusable - apply with `.route_layer(from_fn(require_session))` on whatever
//! sub-router you want protected. On missing/invalid/expired session it returns
//! `401`; on success it stashes the validated [`SessionInfo`] in request
//! extensions so downstream handlers read it via [`axum::Extension`].

use axum::extract::{Extension, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::CookieJar;

use crate::auth::session::{lookup_session, SessionId, SessionInfo, SESSION_COOKIE_NAME};
use crate::error::{Error, Result};
use crate::state::AppState;

/// `axum::middleware::from_fn`-compatible guard. Reads the session cookie,
/// looks the id up in the `sessions` table, refuses if absent/expired, otherwise
/// inserts the validated [`SessionInfo`] into request extensions and forwards.
pub async fn require_session(
    State(state): State<AppState>,
    jar: CookieJar,
    mut request: Request,
    next: Next,
) -> Result<Response> {
    let Some(id_raw) = jar.get(SESSION_COOKIE_NAME).map(|c| c.value().to_string()) else {
        return Err(Error::Unauthorized);
    };
    let Some(id) = SessionId::parse(&id_raw) else {
        return Err(Error::Unauthorized);
    };
    let Some(session) = lookup_session(&state.db, id).await? else {
        return Err(Error::Unauthorized);
    };
    if session.is_expired() {
        return Err(Error::Unauthorized);
    }
    request.extensions_mut().insert(session);
    Ok(next.run(request).await)
}

/// Admin-only guard (issue #73). Runs *behind* [`require_session`] - the
/// validated [`SessionInfo`] is already in request extensions (its `is_admin`
/// flag was sourced from the `users` table by `lookup_session`, so no second
/// DB hit is needed). Refuses non-admin callers with 403; forwards otherwise.
/// Apply with `.route_layer(from_fn(require_admin))` on a sub-router that is
/// itself wrapped by `require_session` (the session must be resolved first so
/// the extension is present).
pub async fn require_admin(
    Extension(session): Extension<SessionInfo>,
    request: Request,
    next: Next,
) -> Result<Response> {
    if !session.is_admin {
        return Err(Error::Forbidden);
    }
    Ok(next.run(request).await)
}
