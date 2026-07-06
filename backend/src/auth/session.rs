//! Opaque session management (issue #2).
//!
//! Replaces the rejected JWT-in-`localStorage` pattern. A session id is a
//! ≥256-bit CSPRNG value minted at login and stored as a row in the `sessions`
//! SQLite table. The id's only transport is an `httpOnly; Secure;
//! SameSite=Strict`, `__Host-`-prefixed cookie; the server is the source of
//! truth for session state. Logout deletes the row.
//!
//! This module is deliberately cookie-unaware at the storage layer — it only
//! mints, looks up, and invalidates ids. The routes attach the cookie.

use base64::Engine;
use rand::rngs::SysRng;
use rand::TryRng;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{now_seconds, Db};
use crate::error::Result;

/// Name of the opaque session cookie.
///
/// The `__Host-` prefix enforces (per RFC 6265bis) that the cookie be `Secure`,
/// `Path=/`, and carry no `Domain` attribute — i.e. scoped to exactly this
/// host. Browsers reject Set-Cookies that violate those constraints, which is
/// the defence we want.
pub const SESSION_COOKIE_NAME: &str = "__Host-sb_session";

/// Number of random bytes in a session id. 256 bits ≫ the 64-bit floor, with
/// room to spare; `URL_SAFE_NO_PAD` encoding yields a 43-char string.
const SESSION_ID_BYTES: usize = 32;

/// Session lifetime in seconds. Personal-scale, single-user: a long-lived
/// session is fine here, but bounded so stale rows can be reaped.
const SESSION_TTL_SECONDS: i64 = 30 * 24 * 60 * 60;

/// A base64url-encoded opaque session id. Construction is the only mint path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionId(String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Decode from the cookie value representation.
    pub fn parse(value: &str) -> Option<Self> {
        // Reject values that aren't a clean base64url encoding of exactly
        // SESSION_ID_BYTES bytes — don't trust the wire.
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(value)
            .ok()?;
        if decoded.len() != SESSION_ID_BYTES {
            return None;
        }
        Some(Self(value.to_string()))
    }
}

/// What a handler learns about a validated session from the middleware /
/// `lookup_session`. `user_id` identifies the account a passkey binds to.
/// Issue #72: `is_admin` and `display_name` are sourced from the `users`
/// table at lookup time so `/me` can return them without a second query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    /// The user's display name from the `users` table (issue #72).
    pub display_name: String,
    /// Whether the user is an admin (issue #72). The bootstrap admin is the
    /// only admin while the singleton lock is in place.
    pub is_admin: bool,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

impl SessionInfo {
    /// True if the session is past its expiry instant. The cookie may still be
    /// on the wire; the middleware uses this to refuse stale sessions.
    pub fn is_expired(&self) -> bool {
        self.expires_at.is_some_and(|exp| now_seconds() > exp)
    }
}

/// base64url (no padding) of `bytes`. Shared by the session-id minter and the
/// WebAuthn challenge-token minter so the two opaque-token encodings stay
/// identical in shape — they're conceptually the same "random then encode".
pub(super) fn base64url_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn encode_id(bytes: &[u8]) -> String {
    base64url_encode(bytes)
}

/// Mint a fresh cryptographically-random bearer token (base64url, no padding,
/// 256 bits of entropy) — the same shape as a session id, but carrying no
/// server-side row of its own until the caller persists it. Used by the admin
/// invite minter (issue #73): an invitation token is a one-time bearer the
/// invitee consumes in a later slice's registration flow.
pub(crate) fn random_bearer_token() -> String {
    let mut rng = SysRng;
    let mut bytes = vec![0u8; SESSION_ID_BYTES];
    rng.try_fill_bytes(&mut bytes)
        .expect("filling bearer token bytes from OS entropy");
    encode_id(&bytes)
}

/// Mint a brand-new, cryptographically-random session id and persist a `sessions`
/// row for it. The id is returned exactly once so the caller can set the cookie;
/// it is never looked up by anything but the cookie value. Issue #72: also
/// looks up the `users` row for `display_name` and `is_admin` so the session
/// carries them for `/me`.
pub async fn mint_session(db: &Db, user_id: &str) -> Result<SessionInfo> {
    let user_id = user_id.to_string();
    db.with_conn(move |conn| {
        let id = random_bearer_token();
        // Uniqueness: 256-bit CSPRNG collision is astronomically unlikely; the
        // PRIMARY KEY is the backstop. On the (impossible) collision we re-roll.
        loop {
            let created_at = now_seconds();
            let expires_at = created_at + SESSION_TTL_SECONDS;
            let inserted = conn.execute(
                "INSERT INTO sessions (session_id, user_id, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, user_id, created_at, expires_at],
            )?;
            if inserted == 1 {
                // Look up the user's display_name and is_admin for the
                // SessionInfo (issue #72). The FK on sessions.user_id →
                // users.id guarantees the row exists.
                let (display_name, is_admin): (String, i64) = conn.query_row(
                    "SELECT display_name, is_admin FROM users WHERE id = ?1",
                    params![user_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                return Ok(SessionInfo {
                    session_id: id,
                    user_id,
                    display_name,
                    is_admin: is_admin != 0,
                    created_at,
                    expires_at: Some(expires_at),
                });
            }
        }
    })
    .await
}

/// Look up a session row by its cookie-presented id. Returns `None` if the row
/// is missing — the middleware turns that into a 401. Issue #72: joins the
/// `users` table so the `SessionInfo` carries `display_name` and `is_admin`.
pub async fn lookup_session(db: &Db, id: SessionId) -> Result<Option<SessionInfo>> {
    let id = id.0;
    db.with_conn(move |conn| {
        let row = conn
            .query_row(
                "SELECT s.session_id, s.user_id, u.display_name, u.is_admin,
                        s.created_at, s.expires_at
                 FROM sessions s
                 JOIN users u ON u.id = s.user_id
                 WHERE s.session_id = ?1",
                params![id],
                |row| {
                    Ok(SessionInfo {
                        session_id: row.get(0)?,
                        user_id: row.get(1)?,
                        display_name: row.get(2)?,
                        is_admin: row.get::<_, i64>(3)? != 0,
                        created_at: row.get(4)?,
                        expires_at: row.get(5)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    })
    .await
}

/// Invalidate a session by deleting its row. Idempotent: deleting a row that's
/// already gone affected zero rows and is still a success.
pub async fn invalidate_session(db: &Db, id: &SessionId) -> Result<()> {
    let id = id.as_str().to_string();
    db.with_conn(move |conn| {
        conn.execute("DELETE FROM sessions WHERE session_id = ?1", params![id])?;
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_round_trips_through_cookie_value() {
        let bytes = [0x42u8; SESSION_ID_BYTES];
        let encoded = encode_id(&bytes);
        let parsed = SessionId::parse(&encoded).expect("parses");
        assert_eq!(parsed.as_str(), encoded);
    }

    #[test]
    fn session_id_rejects_garbage() {
        // Wrong length after decode.
        let short = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([0u8; 16]);
        assert!(SessionId::parse(&short).is_none());
        // Non-base64.
        assert!(SessionId::parse("not~~~~base64!!!").is_none());
    }

    #[tokio::test]
    async fn mint_then_lookup_then_invalidate() {
        let db = Db::open_in_memory().unwrap();
        let s = mint_session(&db, "00000000-0000-0000-0000-000000000001")
            .await
            .unwrap();
        let id = SessionId::parse(&s.session_id).unwrap();
        let found = lookup_session(&db, id.clone()).await.unwrap().unwrap();
        assert_eq!(found.user_id, "00000000-0000-0000-0000-000000000001");
        assert_eq!(found.display_name, "me");
        assert!(found.is_admin, "bootstrap admin must be is_admin");
        assert!(!found.is_expired());

        invalidate_session(&db, &id).await.unwrap();
        let gone = lookup_session(&db, id.clone()).await.unwrap();
        assert!(gone.is_none(), "session row must be gone after logout");
    }

    #[tokio::test]
    async fn invalidated_session_id_is_not_reused_for_a_new_mint() {
        // Mint two sessions and confirm distinct ids — sanity check SysRng.
        let db = Db::open_in_memory().unwrap();
        let a = mint_session(&db, "00000000-0000-0000-0000-000000000001")
            .await
            .unwrap();
        let b = mint_session(&db, "00000000-0000-0000-0000-000000000001")
            .await
            .unwrap();
        assert_ne!(a.session_id, b.session_id);
    }
}
