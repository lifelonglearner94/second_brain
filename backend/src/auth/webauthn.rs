//! WebAuthn (passkey) registration + login against `webauthn-rs`.
//!
//! Two-phase challenge flows: a begin call mints a server-side state value and
//! a public `CreationChallengeResponse` / `RequestChallengeResponse`; the
//! finish call pairs the client's assertion with the stored state and either
//! persists a credential (register) or mints a session (login).
//!
//! State is held in an in-memory, TTL-bounded `ChallengeStore` keyed by an
//! opaque, single-use token the client echoes back. Personal scale: a
//! process-local map is the right shape. If the server restarts mid flow the
//! user simply re-starts - no session is poisoned.
//!
//! Issue #74 replaces the deploy-time "first passkey wins" singleton lock with
//! invitation-gated registration plus a one-time bootstrap exception:
//! - **Bootstrap exception**: when zero users exist (`SELECT COUNT(*) FROM
//!   users == 0`), the first `register_begin`/`register_finish` proceeds with
//!   no invitation and creates the admin (`is_admin = true`). The exception
//!   closes the moment any user exists.
//! - **Invite-gated registration**: once the admin exists, every registration
//!   must present a valid, unconsumed invitation token. `register_begin`
//!   validates the token (404 unknown, 410 consumed) and binds it to the
//!   challenge state; `register_finish` consumes the invite atomically under
//!   the single-connection mutex, creates a fresh non-admin `users` row, seeds
//!   its day-zero ontology, binds the passkey, and mints a session. A consumed
//!   or reused invite is refused with 410; an unknown invite with 404.
//!
//! Login is unchanged in shape but now resolves the user from the
//! authenticated passkey row (`SELECT user_id FROM passkeys WHERE cred_id = ?`)
//! rather than the old `SINGLE_USER_ID` constant, so the resulting session
//! carries the real, per-user `user_id` the domain layer scopes by.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::{params, OptionalExtension};
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder,
};

use crate::db::{now_seconds, seed_ontology_for_user_conn, Db, BOOTSTRAP_ADMIN_USER_ID};
use crate::error::{Error, Result};

/// Display name / friendly name for the bootstrap admin's WebAuthn challenge.
/// The bootstrap registration is the only path that uses these - invitees get a
/// generic placeholder name (issue #74).
const BOOTSTRAP_USER_NAME: &str = "me";
const BOOTSTRAP_USER_DISPLAY_NAME: &str = "me";

/// Display name for an invite-registered (non-admin) user. A later slice may
/// let the invitee choose a name; for now a neutral placeholder is fine.
const INVITEE_DISPLAY_NAME: &str = "user";

/// How long a begin-state stays valid before its finish must arrive.
const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);

/// Token mint / lookup key. 32 bytes of OsRNG, base64url - same rules as a
/// session id but with a separate namespace (it never leaves the begin/finish
/// pair), so a distinct type keeps them from being confused at call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeToken(String);

impl ChallengeToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Context bound to a pending registration so `register_finish` knows which
/// path to take (bootstrap vs invite) and which account to create. Carried
/// alongside the WebAuthn `PasskeyRegistration` in the challenge store.
#[derive(Debug)]
struct RegisterContext {
    state: PasskeyRegistration,
    /// The `users.id` to create at finish - `BOOTSTRAP_ADMIN_USER_ID` for the
    /// bootstrap path, a fresh `Uuid::new_v4()` for the invite path.
    user_id: String,
    /// Whether the new row is the admin. True only for the bootstrap path.
    is_admin: bool,
    /// Display name for the new `users` row.
    display_name: String,
    /// The invitation token to consume at finish. `None` for the bootstrap
    /// path; `Some(token)` for the invite path.
    invite_token: Option<String>,
}

enum ChallengeState {
    Register(RegisterContext),
    Login(PasskeyAuthentication),
}

/// In-memory, TTL-bounded store of pending challenges. Locked under a `Mutex`
/// because begin/finish arrive on different async tasks; the critical section
/// is a HashMap touch, never a WebAuthn call (those run outside the lock).
#[derive(Default)]
pub struct ChallengeStore {
    inner: Mutex<HashMap<String, (ChallengeState, Instant)>>,
}

impl ChallengeStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn purge_expired(map: &mut HashMap<String, (ChallengeState, Instant)>) {
        let now = Instant::now();
        map.retain(|_, (_, exp)| *exp > now);
    }

    /// Insert a state and return the token the client must echo.
    fn put(&self, state: ChallengeState) -> ChallengeToken {
        use rand::TryRng;
        let mut bytes = vec![0u8; 32];
        rand::rngs::SysRng
            .try_fill_bytes(&mut bytes)
            .expect("filling challenge bytes from OS entropy");
        let token = super::session::base64url_encode(&bytes);

        let mut map = self.inner.lock().expect("challenge store poisoned");
        Self::purge_expired(&mut map);
        map.insert(token.clone(), (state, Instant::now() + CHALLENGE_TTL));
        ChallengeToken(token)
    }

    /// Take the state for `token`, removing it (challenges are single-use).
    /// Returns an error if the token is unknown, expired, or the wrong phase.
    fn take_register(&self, token: &str) -> Result<RegisterContext> {
        let mut map = self.inner.lock().expect("challenge store poisoned");
        Self::purge_expired(&mut map);
        match map.remove(token) {
            Some((ChallengeState::Register(ctx), _)) => Ok(ctx),
            _ => Err(Error::BadRequest(
                "unknown or expired registration state".into(),
            )),
        }
    }

    fn take_login(&self, token: &str) -> Result<PasskeyAuthentication> {
        let mut map = self.inner.lock().expect("challenge store poisoned");
        Self::purge_expired(&mut map);
        match map.remove(token) {
            Some((ChallengeState::Login(s), _)) => Ok(s),
            _ => Err(Error::BadRequest("unknown or expired login state".into())),
        }
    }
}

/// Build the `Webauthn` instance from environment-driven relying-party config.
/// Errors here are fatal: the caller (main / tests) should propagate.
pub fn build_webauthn(rp_id: &str, rp_origin: &str, rp_name: &str) -> Result<Webauthn> {
    let origin = Url::parse(rp_origin)
        .map_err(|e| Error::Internal(format!("invalid WEBAUTHN_RP_ORIGIN {rp_origin:?}: {e}")))?;
    let builder = WebauthnBuilder::new(rp_id, &origin)?;
    let webauthn = builder.rp_name(rp_name).build()?;
    Ok(webauthn)
}

/// Result of a registration-begin: the public challenge the client signs, plus
/// the opaque `state` token the client echoes on finish.
#[derive(Debug, serde::Serialize)]
pub struct RegistrationBegin {
    /// The WebAuthn creation options - forward `publicKey` verbatim to the
    /// browser's `navigator.credentials.create`.
    pub challenge: webauthn_rs_proto::CreationChallengeResponse,
    /// Opaque state token; send it back in the finish request body.
    pub state: String,
}

/// Body the client posts to begin registration. Issue #74: the optional
/// `invite` is the admin-issued invitation token. When the bootstrap exception
/// is open (zero users), `invite` is ignored; otherwise it is required and
/// validated (404 unknown, 410 consumed).
#[derive(Debug, serde::Deserialize, Default)]
pub struct RegistrationBeginRequest {
    pub invite: Option<String>,
}

/// Body the client posts to finish registration.
#[derive(Debug, serde::Deserialize)]
pub struct RegistrationFinish {
    /// The `RegisterPublicKeyCredential` the browser produced.
    pub credential: RegisterPublicKeyCredential,
    /// The `state` token from the matching begin call.
    pub state: String,
}

/// Result of a successful login-begin.
#[derive(Debug, serde::Serialize)]
pub struct LoginBegin {
    /// The WebAuthn request options - forward `publicKey` verbatim to the
    /// browser's `navigator.credentials.get`.
    pub challenge: webauthn_rs_proto::RequestChallengeResponse,
    /// Opaque state token; send it back in the finish request body.
    pub state: String,
}

/// Body the client posts to finish login.
#[derive(Debug, serde::Deserialize)]
pub struct LoginFinish {
    pub credential: PublicKeyCredential,
    pub state: String,
}

/// Result of a successful login: the per-user `user_id` the authenticated
/// passkey binds to (resolved from the `passkeys` row, issue #74). The route
/// mints a session for this id.
#[derive(Debug)]
pub struct LoginResult {
    pub user_id: String,
}

/// High-level auth façade - owns the `Webauthn` instance and the challenge
/// store, talks to the `Db` for credential persistence. Cloned cheaply into
/// the shared `AppState`.
#[derive(Clone)]
pub struct AuthService {
    pub webauthn: Webauthn,
    pub challenges: std::sync::Arc<ChallengeStore>,
}

impl AuthService {
    pub fn new(webauthn: Webauthn) -> Self {
        Self {
            webauthn,
            challenges: std::sync::Arc::new(ChallengeStore::new()),
        }
    }

    /// Begin passkey registration. Issue #74: invite-gated with a bootstrap
    /// exception. When zero users exist, proceeds with no invitation (the
    /// bootstrap path that will create the admin at finish). Otherwise
    /// requires a valid, unconsumed `invite` token and validates it (404
    /// unknown, 410 consumed). The chosen `user_id` and registration context
    /// are bound to the challenge state so finish is self-contained.
    pub async fn register_begin(
        &self,
        db: &Db,
        invite: Option<String>,
    ) -> Result<RegistrationBegin> {
        let user_count = count_users(db).await?;
        let (user_id, is_admin, display_name, invite_token) = if user_count == 0 {
            // Bootstrap exception: zero users → create the admin, no invite.
            (
                BOOTSTRAP_ADMIN_USER_ID.to_string(),
                true,
                BOOTSTRAP_USER_DISPLAY_NAME.to_string(),
                None,
            )
        } else {
            // Invite-gated: a valid, unconsumed token is required.
            let token = invite.ok_or_else(|| {
                Error::BadRequest("an invitation token is required to register".into())
            })?;
            validate_invite(db, &token).await?; // 404 unknown, 410 consumed
            (
                Uuid::new_v4().to_string(),
                false,
                INVITEE_DISPLAY_NAME.to_string(),
                Some(token),
            )
        };

        let user_unique_id = Uuid::parse_str(&user_id)
            .map_err(|e| Error::Internal(format!("invalid user_id uuid: {e}")))?;
        let (user_name, user_display_name) = if is_admin {
            (BOOTSTRAP_USER_NAME, BOOTSTRAP_USER_DISPLAY_NAME)
        } else {
            (INVITEE_DISPLAY_NAME, INVITEE_DISPLAY_NAME)
        };
        let webauthn = self.webauthn.clone();
        let (challenge, state) = webauthn.start_passkey_registration(
            user_unique_id,
            user_name,
            user_display_name,
            // No exclude-credentials: each passkey is independent (device-loss
            // tolerance) and now each invitee is a distinct user.
            None,
        )?;

        let context = RegisterContext {
            state,
            user_id,
            is_admin,
            display_name,
            invite_token,
        };
        let token = self.challenges.put(ChallengeState::Register(context));
        Ok(RegistrationBegin {
            challenge,
            state: token.as_str().to_string(),
        })
    }

    /// Finish passkey registration: pair the client's credential with the
    /// stored state, then atomically (under the single-connection mutex) either
    /// create the bootstrap admin (exception path, re-checked against
    /// `COUNT(users) == 0`) or consume the invite and create a fresh non-admin
    /// user. Binds the passkey, seeds the new user's day-zero ontology, and
    /// mints a session. Single-use - the state token is consumed.
    pub async fn register_finish(
        &self,
        db: &Db,
        body: RegistrationFinish,
    ) -> Result<crate::auth::session::SessionInfo> {
        let ctx = self.challenges.take_register(&body.state)?;
        let webauthn = self.webauthn.clone();
        // `finish_passkey_registration` is cheap CPU crypto on already-received
        // data; park on the calling task (no spawn_blocking needed).
        let passkey = webauthn.finish_passkey_registration(&body.credential, &ctx.state)?;
        let cred_id = passkey.cred_id().to_vec();
        let passkey_json = serde_json::to_string(&passkey)
            .map_err(|e| Error::Internal(format!("passkey serialization: {e}")))?;

        let user_id = ctx.user_id.clone();
        let is_admin = ctx.is_admin;
        let display_name = ctx.display_name.clone();
        let invite_token = ctx.invite_token.clone();
        // Keep a copy for the post-closure session mint (the closure captures
        // `user_id` by move).
        let user_id_for_session = user_id.clone();
        // The whole finish is one transaction so a failed consume (e.g. invite
        // already consumed between begin and finish) rolls back the half-created
        // `users` row. The single-connection mutex serializes this against every
        // other `with_conn` closure, so the consume is race-proof. The
        // `invitations.consumed_by_user_id` FK requires the `users` row to exist
        // first, so we create the user before consuming the invite.
        db.with_conn(move |conn| {
            conn.execute_batch("BEGIN")?;
            let work: Result<()> = (|| {
                // Bootstrap path: re-check the exception under the mutex so two
                // racing bootstrap finishes can't both create the admin. The
                // invite path skips this (the invite is the gate).
                if invite_token.is_none() {
                    let count: i64 =
                        conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?;
                    if count > 0 {
                        return Err(Error::Conflict(
                            "bootstrap exception closed: an admin already exists".into(),
                        ));
                    }
                }
                // Create the users row first (FK target for the invite consume).
                conn.execute(
                    "INSERT INTO users (id, display_name, is_admin, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        user_id,
                        display_name,
                        if is_admin { 1 } else { 0 },
                        now_seconds()
                    ],
                )?;
                // Consume the invite now that the user exists (atomic against
                // other `with_conn` closures under the mutex). 0 rows means the
                // invite was unknown or already consumed between begin and finish.
                if let Some(token) = &invite_token {
                    let rows = conn.execute(
                        "UPDATE invitations
                            SET status = 'consumed',
                                consumed_at = ?1,
                                consumed_by_user_id = ?2
                          WHERE token = ?3 AND status = 'pending'",
                        params![now_seconds(), user_id, token],
                    )?;
                    if rows == 0 {
                        let exists: i64 = conn.query_row(
                            "SELECT COUNT(*) FROM invitations WHERE token = ?1",
                            params![token],
                            |r| r.get(0),
                        )?;
                        if exists == 0 {
                            return Err(Error::NotFound("unknown invitation token".into()));
                        }
                        return Err(Error::Gone("invitation already consumed".into()));
                    }
                }
                // Seed the new user's day-zero ontology (idempotent).
                seed_ontology_for_user_conn(conn, &user_id)?;
                // Bind the passkey to the new user.
                conn.execute(
                    "INSERT INTO passkeys (cred_id, user_id, passkey_json, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![cred_id, user_id, passkey_json, now_seconds()],
                )?;
                Ok(())
            })();
            match work {
                Ok(()) => {
                    conn.execute_batch("COMMIT")?;
                    Ok(())
                }
                Err(e) => {
                    // Best-effort rollback; the error is the real result.
                    let _ = conn.execute_batch("ROLLBACK");
                    Err(e)
                }
            }
        })
        .await?;

        // Mint a session for the freshly created user so registration logs the
        // invitee in immediately (the route sets the cookie).
        crate::auth::session::mint_session(db, &user_id_for_session).await
    }

    pub async fn login_begin(&self, db: &Db) -> Result<LoginBegin> {
        // Issue #74: load every passkey across all users so any registered
        // account can initiate login. The user is resolved at finish from the
        // credential id, not assumed from a constant.
        let passkeys = load_all_passkeys(db).await?;
        if passkeys.is_empty() {
            return Err(Error::NotFound("no registered passkey".into()));
        }
        let webauthn = self.webauthn.clone();
        let (challenge, state) = webauthn.start_passkey_authentication(&passkeys)?;
        let token = self.challenges.put(ChallengeState::Login(state));
        Ok(LoginBegin {
            challenge,
            state: token.as_str().to_string(),
        })
    }

    pub async fn login_finish(&self, db: &Db, body: LoginFinish) -> Result<LoginResult> {
        let state = self.challenges.take_login(&body.state)?;
        let webauthn = self.webauthn.clone();
        let result = webauthn.finish_passkey_authentication(&body.credential, &state)?;
        // Issue #74: resolve the per-user user_id from the authenticated
        // passkey row (the credential id uniquely identifies a passkey, which
        // belongs to exactly one user). This is what scopes the minted session.
        let cred_id = result.cred_id().to_vec();
        let user_id = resolve_user_for_credential(db, &cred_id).await?;
        // Counter check: if the authenticator reports a counter we must ensure
        // it advanced past the stored value (cloned-credential defence).
        update_passkey_counter(db, &user_id, &result).await?;
        Ok(LoginResult { user_id })
    }
}

/// `SELECT COUNT(*) FROM users` - the bootstrap-exception gate (issue #74).
/// When this is zero, `register_begin` proceeds with no invitation and creates
/// the admin; the moment it is non-zero, the exception closes and registration
/// requires an invite.
async fn count_users(db: &Db) -> Result<i64> {
    db.with_conn(|conn| Ok(conn.query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))?))
        .await
}

/// Validate that `token` is a known, pending invitation. Returns `Ok(())` if
/// so, `Error::NotFound` if the token matches no row (unknown invite), and
/// `Error::Gone` if the row exists but is already consumed. Used by
/// `register_begin` for the early, best-effort validation; the authoritative,
/// race-proof consume lives in `register_finish` under the mutex.
async fn validate_invite(db: &Db, token: &str) -> Result<()> {
    let token = token.to_string();
    db.with_conn(move |conn| {
        let status: Option<String> = conn
            .query_row(
                "SELECT status FROM invitations WHERE token = ?1",
                params![token],
                |r| r.get(0),
            )
            .optional()?;
        match status {
            Some(s) if s == "pending" => Ok(()),
            Some(_) => Err(Error::Gone("invitation already consumed".into())),
            None => Err(Error::NotFound("unknown invitation token".into())),
        }
    })
    .await
}

/// Resolve the `user_id` for the passkey whose `cred_id` authenticated. The
/// credential id uniquely identifies a passkey row, which belongs to exactly
/// one user. Returns `Error::NotFound` if no row matches (shouldn't happen
/// after a verified authentication, but treated as a clean error rather than
/// a panic).
async fn resolve_user_for_credential(db: &Db, cred_id: &[u8]) -> Result<String> {
    let cred_id = cred_id.to_vec();
    db.with_conn(move |conn| {
        let user_id: String = conn
            .query_row(
                "SELECT user_id FROM passkeys WHERE cred_id = ?1",
                params![cred_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound("authenticated credential not found".into()))?;
        Ok(user_id)
    })
    .await
}

/// Re-persist a passkey after its counter advanced on re-auth (the cloned-
/// credential defence in [`update_passkey_counter`]). An upsert: the cred_id
/// already exists, so `ON CONFLICT … DO UPDATE` refreshes the JSON.
async fn upsert_passkey(db: &Db, user_id: &str, passkey: &Passkey) -> Result<()> {
    let cred_id = passkey.cred_id().to_vec();
    let passkey_json = serde_json::to_string(passkey)
        .map_err(|e| Error::Internal(format!("passkey serialization: {e}")))?;
    let user_id = user_id.to_string();
    db.with_conn(move |conn| {
        conn.execute(
            "INSERT INTO passkeys (cred_id, user_id, passkey_json, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(cred_id) DO UPDATE SET passkey_json = excluded.passkey_json",
            params![cred_id, user_id, passkey_json, now_seconds()],
        )?;
        Ok(())
    })
    .await
}

/// Load every passkey across all users (issue #74: login is no longer scoped to
/// a single constant user). Empty vec → login-begin refuses (no credential to
/// authenticate against). JSON deserialization is done off the blocking thread
/// so a corrupted row surfaces as a clean internal error.
async fn load_all_passkeys(db: &Db) -> Result<Vec<Passkey>> {
    let jsons: Vec<String> = db
        .with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT passkey_json FROM passkeys ORDER BY created_at")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await?;
    let mut passkeys = Vec::with_capacity(jsons.len());
    for json in jsons {
        let pk: Passkey = serde_json::from_str(&json)
            .map_err(|e| Error::Internal(format!("passkey deserialization: {e}")))?;
        passkeys.push(pk);
    }
    Ok(passkeys)
}

/// Update the stored counter for the credential that just authenticated, per
/// the spec's cloned-credential check. We load the matching user's passkeys,
/// ask the library to fold the authentication result into it
/// (`update_credential` advances the counter internally), and re-persist. Pure-
/// server counter inspection isn't possible - `Passkey.cred.counter` is
/// `pub(crate)` - so the library's own mutator is the only safe handle.
///
/// A counter that didn't advance (cloned-credential signal) is logged but not
/// auto-invalidated: at personal scale we surface it to the human via logs
/// rather than lock the user out automatically.
async fn update_passkey_counter(
    db: &Db,
    user_id: &str,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> Result<()> {
    let passkeys = load_passkeys_for_user(db, user_id).await?;
    for mut pk in passkeys {
        if let Some(changed) = pk.update_credential(result) {
            if changed {
                upsert_passkey(db, user_id, &pk).await?;
            } else {
                tracing::warn!(
                    cred_id = ?pk.cred_id(),
                    "passkey counter did not advance; possible cloned credential"
                );
            }
            return Ok(());
        }
    }
    // No matching credential id - shouldn't happen since finish verified it,
    // but treat as a benign no-op rather than failing a verified login.
    tracing::warn!("authenticated credential not found in passkey store");
    Ok(())
}

/// Load every passkey registered for `user_id` (used by the counter update on
/// re-auth). JSON deserialization is done off the blocking thread.
async fn load_passkeys_for_user(db: &Db, user_id: &str) -> Result<Vec<Passkey>> {
    let user_id = user_id.to_string();
    let jsons: Vec<String> = db
        .with_conn(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT passkey_json FROM passkeys WHERE user_id = ?1 ORDER BY created_at",
            )?;
            let rows = stmt.query_map(params![user_id], |row| row.get::<_, String>(0))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r?);
            }
            Ok(out)
        })
        .await?;
    let mut passkeys = Vec::with_capacity(jsons.len());
    for json in jsons {
        let pk: Passkey = serde_json::from_str(&json)
            .map_err(|e| Error::Internal(format!("passkey deserialization: {e}")))?;
        passkeys.push(pk);
    }
    Ok(passkeys)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_webauthn_accepts_localhost() {
        let w = build_webauthn("localhost", "http://localhost:8080", "test");
        assert!(w.is_ok(), "localhost should be a valid RP id/origin");
    }

    #[test]
    fn build_webauthn_rejects_mismatched_rp_id() {
        let w = build_webauthn("example.com", "http://localhost:8080", "test");
        assert!(w.is_err());
    }
}
