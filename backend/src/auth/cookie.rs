//! Cookie construction for the session. The cookie is the *only* transport
//! for the opaque session id and carries none of the session's meaning - the
//! `sessions` SQLite row is the source of truth.
//!
//! Semantics, per `first_draft.md` §A:
//! * `httpOnly` - no JS access, so an XSS cannot exfiltrate the id.
//! * `Secure` - only ever sent over HTTPS in production.
//! * `SameSite=Strict` - never sent on cross-site requests (CSRF defence).
//! * `__Host-` prefix - pin to this host: no `Domain`, `Path=/`, `Secure`.
//!
//! In tests we craft `Cookie:` headers by hand; the server doesn't re-check
//! `Secure` on read (that's a browser-wire concern), so http test clients work.

use axum::http::header::SET_COOKIE;
use axum::http::{HeaderName, HeaderValue};
use axum::response::{AppendHeaders, IntoResponse, Response};
use cookie::{Cookie, SameSite};

use crate::auth::session::{SessionId, SESSION_COOKIE_NAME};

/// Lifetime in seconds to set on the cookie, mirroring the row's expiry so the
/// browser discards the cookie around when the row becomes stale/reapable.
const COOKIE_MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;

/// Build a `Set-Cookie` header value carrying `id` (or clearing it when `None`).
fn build<'a>(id: Option<&'a str>) -> Cookie<'a> {
    let (value, max_age) = match id {
        Some(v) => (v, cookie::time::Duration::seconds(COOKIE_MAX_AGE_SECS)),
        // Empty value + Max-Age=0 is the canonical "delete this cookie".
        None => ("", cookie::time::Duration::ZERO),
    };
    Cookie::build((SESSION_COOKIE_NAME, value))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .path("/")
        .max_age(max_age)
        .build()
}

/// `(name, value)` pair the `AppendHeaders` builder expects.
fn set_cookie_pair(value: String) -> (HeaderName, HeaderValue) {
    // Unwrap: the cookie string is ASCII-safe (base64url id + fixed attrs).
    (
        SET_COOKIE,
        HeaderValue::from_str(&value).expect("valid cookie header"),
    )
}

/// Header parts (for wrapping a handler's OK response) that set a fresh
/// session cookie carrying `id`. Use with `(session_cookie_headers(&id), body)`.
pub fn session_cookie_headers(id: &SessionId) -> AppendHeaders<[(HeaderName, HeaderValue); 1]> {
    AppendHeaders([set_cookie_pair(build(Some(id.as_str())).to_string())])
}

/// A response whose only effect is clearing the session cookie (used on logout).
pub fn clear_cookie_response<R: IntoResponse>(body: R) -> Response {
    let headers = AppendHeaders([set_cookie_pair(build(None).to_string())]);
    (headers, body).into_response()
}

/// Header-only helper for tests that want to send a session id back to the
/// server: builds a `Cookie: __Host-sb_session=<value>` header value.
pub fn request_cookie_header_value(id: &SessionId) -> HeaderValue {
    let raw = format!("{}={}", SESSION_COOKIE_NAME, id.as_str());
    HeaderValue::from_str(&raw).expect("valid cookie header")
}
