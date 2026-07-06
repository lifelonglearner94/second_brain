//! Authentication: WebAuthn passkey registration/login + opaque session
//! cookies (issue #2). Replaces the rejected JWT-in-`localStorage` pattern.
//!
//! Two responsibilities, one module each:
//! * [`session`] mints, looks up, and invalidates opaque server-side session
//!   ids (the cookie is merely a bearer of an opaque id).
//! * [`webauthn`] drives the WebAuthn begin/finish flows and persists
//!   registered passkeys in SQLite.
//!
//! Cookie semantics (`httpOnly; Secure; SameSite=Strict`, `__Host-`-prefixed)
//! live in [`cookie`], which the routes use to attach and clear the cookie.

pub mod cookie;
pub mod middleware;
pub mod session;
pub mod webauthn;

pub use middleware::require_admin;
pub use session::{invalidate_session, lookup_session, mint_session, SessionId, SessionInfo};
pub use webauthn::{
    build_webauthn, AuthService, LoginBegin, LoginFinish, LoginResult, RegistrationBegin,
    RegistrationBeginRequest, RegistrationFinish,
};
