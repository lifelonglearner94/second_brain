//! WebAuthn (passkey) registration + login against `webauthn-rs`.
//!
//! Two-phase challenge flows: a begin call mints a server-side state value and
//! a public `CreationChallengeResponse` / `RequestChallengeResponse`; the
//! finish call pairs the client's assertion with the stored state and either
//! persists a credential (register) or mints a session (login).
//!
//! State is held in an in-memory, TTL-bounded `ChallengeStore` keyed by an
//! opaque, single-use token the client echoes back. Personal-scale, single
//! user: a process-local map is the right shape. If the server restarts mid
//! flow the user simply re-starts — no session is poisoned.
//!
//! Registration is gated by a deploy-time singleton lock (issue #2): once one
//! passkey exists, registration is closed. Enforced at `register_begin`
//! (best-effort, so a second caller gets a clean 409 before the WebAuthn
//! dance) and authoritatively inside `store_first_passkey` — count + insert
//! share one `db.run` closure, so under the single-connection mutex the gate
//! is race-proof even if two begins both succeed before either finish lands.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Url, Uuid, Webauthn, WebauthnBuilder,
};

use crate::db::{now_seconds, Db};
use crate::error::{Error, Result};

/// The single account a registered passkey binds to. This app is single-user;
/// a real multi-tenant split would source this from a users table. We use a
/// stable UUID so passkeys stay associated with the same account across
/// restarts — the literal id is arbitrary but must be constant.
const SINGLE_USER_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Friendly name shown on the authenticator's UI; arbitrary but constant.
const SINGLE_USER_NAME: &str = "me";
const SINGLE_USER_DISPLAY_NAME: &str = "me";

/// How long a begin-state stays valid before its finish must arrive.
const CHALLENGE_TTL: Duration = Duration::from_secs(5 * 60);

/// Token mint / lookup key. 32 bytes of OsRNG, base64url — same rules as a
/// session id but with a separate namespace (it never leaves the begin/finish
/// pair), so a distinct type keeps them from being confused at call sites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChallengeToken(String);

impl ChallengeToken {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The server-side half of a begin/finish pair. Held only until the matching
/// finish arrives (or the TTL expires).
enum ChallengeState {
    Register(PasskeyRegistration),
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
        use rand::RngCore;
        let mut bytes = vec![0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        let token = super::session::base64url_encode(&bytes);

        let mut map = self.inner.lock().expect("challenge store poisoned");
        Self::purge_expired(&mut map);
        map.insert(token.clone(), (state, Instant::now() + CHALLENGE_TTL));
        ChallengeToken(token)
    }

    /// Take the state for `token`, removing it (challenges are single-use).
    /// Returns an error if the token is unknown, expired, or the wrong phase.
    fn take_register(&self, token: &str) -> Result<PasskeyRegistration> {
        let mut map = self.inner.lock().expect("challenge store poisoned");
        Self::purge_expired(&mut map);
        match map.remove(token) {
            Some((ChallengeState::Register(s), _)) => Ok(s),
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
    /// The WebAuthn creation options — forward `publicKey` verbatim to the
    /// browser's `navigator.credentials.create`.
    pub challenge: webauthn_rs_proto::CreationChallengeResponse,
    /// Opaque state token; send it back in the finish request body.
    pub state: String,
}

/// Body the client posts to finish registration.
#[derive(Debug, serde::Deserialize)]
pub struct RegistrationFinish {
    /// The `RegisterPublicKeyCredential` the browser produced.
    pub credential: RegisterPublicKeyCredential,
    /// The `state` token from the matching begin call.
    pub state: String,
}

/// Result of a login-begin.
#[derive(Debug, serde::Serialize)]
pub struct LoginBegin {
    /// The WebAuthn request options — forward `publicKey` verbatim to the
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

/// High-level auth façade — owns the `Webauthn` instance and the challenge
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

    /// The account id all registered passkeys attach to.
    pub fn user_id(&self) -> &'static str {
        SINGLE_USER_ID
    }

    pub async fn register_begin(&self, db: &Db) -> Result<RegistrationBegin> {
        // Deploy-time singleton lock — best-effort check at begin so a second
        // caller gets a clean 409 before the WebAuthn dance. The authoritative,
        // race-proof gate lives in `store_first_passkey` (count + insert under
        // one DB-closure hold); this one only closes the door early for UX.
        let count = {
            let user_id = SINGLE_USER_ID.to_string();
            db.run(move |conn| count_passkeys_sync(conn, &user_id))
                .await?
        };
        if count > 0 {
            return Err(Error::Conflict(
                "registration is closed: a passkey already exists".into(),
            ));
        }
        let user_unique_id = Uuid::parse_str(SINGLE_USER_ID)
            .map_err(|e| Error::Internal(format!("bad SINGLE_USER_ID: {e}")))?;
        // Show no already-registered credentials to exclude — we want each
        // passkey independent (device-loss tolerance). This is single-user.
        let webauthn = self.webauthn.clone();
        let (challenge, state) = webauthn.start_passkey_registration(
            user_unique_id,
            SINGLE_USER_NAME,
            SINGLE_USER_DISPLAY_NAME,
            None,
        )?;
        let token = self.challenges.put(ChallengeState::Register(state));
        Ok(RegistrationBegin {
            challenge,
            state: token.as_str().to_string(),
        })
    }

    pub async fn register_finish(&self, db: &Db, body: RegistrationFinish) -> Result<()> {
        let state = self.challenges.take_register(&body.state)?;
        let webauthn = self.webauthn.clone();
        // `finish_passkey_registration` is cheap CPU work but it does crypto;
        // park on the calling task. It's not Send in a way that needs
        // spawn_blocking — it's pure computation on already-received data.
        let passkey = webauthn.finish_passkey_registration(&body.credential, &state)?;
        store_first_passkey(db, SINGLE_USER_ID, &passkey).await
    }

    pub async fn login_begin(&self, db: &Db) -> Result<LoginBegin> {
        let passkeys = load_passkeys(db, SINGLE_USER_ID).await?;
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

    pub async fn login_finish(
        &self,
        db: &Db,
        body: LoginFinish,
    ) -> Result<webauthn_rs::prelude::AuthenticationResult> {
        let state = self.challenges.take_login(&body.state)?;
        let webauthn = self.webauthn.clone();
        let result = webauthn.finish_passkey_authentication(&body.credential, &state)?;
        // Counter check: if the authenticator reports a counter we must ensure
        // it advanced past the stored value (cloned-credential defence).
        update_passkey_counter(db, SINGLE_USER_ID, &result).await?;
        Ok(result)
    }
}

/// Count registered passkeys for `user_id`. Drives the deploy-time singleton
/// lock (issue #2): once a passkey exists, registration is closed. Synchronous
/// so it can be folded into a `db.run` closure alongside the insert — the
/// single-connection mutex makes the count + insert atomic w.r.t. other
/// `db.run` calls, which is what makes the lock race-proof.
fn count_passkeys_sync(conn: &Connection, user_id: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM passkeys WHERE user_id = ?1",
        params![user_id],
        |row| row.get(0),
    )?)
}

/// Persist the first registered passkey, gated by the singleton lock. The
/// count check and the insert share one `db.run` closure so no second
/// `register_finish` can slip between them — the authoritative gate, as
/// opposed to the best-effort check in [`AuthService::register_begin`]. A
/// plain `INSERT` (no `ON CONFLICT`) is correct here: count == 0 means the
/// cred_id cannot already exist. Used only by `register_finish`; counter
/// updates on re-auth go through [`upsert_passkey`].
async fn store_first_passkey(db: &Db, user_id: &str, passkey: &Passkey) -> Result<()> {
    let cred_id = passkey.cred_id().to_vec();
    let passkey_json = serde_json::to_string(passkey)
        .map_err(|e| Error::Internal(format!("passkey serialization: {e}")))?;
    let user_id = user_id.to_string();
    db.run(move |conn| {
        if count_passkeys_sync(conn, &user_id)? > 0 {
            return Err(Error::Conflict(
                "registration is closed: a passkey already exists".into(),
            ));
        }
        conn.execute(
            "INSERT INTO passkeys (cred_id, user_id, passkey_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![cred_id, user_id, passkey_json, now_seconds()],
        )?;
        Ok(())
    })
    .await
}

/// Re-persist a passkey after its counter advanced on re-auth (the cloned-
/// credential defence in [`update_passkey_counter`]). An upsert, not a first
/// insert: the cred_id already exists, so `ON CONFLICT … DO UPDATE` refreshes
/// the JSON. Deliberately does **not** re-check the singleton lock — the
/// credential is already registered; this is a counter refresh, not a new
/// registration.
async fn upsert_passkey(db: &Db, user_id: &str, passkey: &Passkey) -> Result<()> {
    let cred_id = passkey.cred_id().to_vec();
    let passkey_json = serde_json::to_string(passkey)
        .map_err(|e| Error::Internal(format!("passkey serialization: {e}")))?;
    let user_id = user_id.to_string();
    db.run(move |conn| {
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

/// Load every passkey registered for `user_id`. Empty vec → login-begin refuses
/// (a user with no credential cannot start an auth). JSON deserialization is
/// done off the blocking thread so a corrupted row surfaces as a clean
/// internal error rather than a rusqlite conversion failure.
async fn load_passkeys(db: &Db, user_id: &str) -> Result<Vec<Passkey>> {
    let user_id = user_id.to_string();
    let jsons: Vec<String> = db
        .run(move |conn| {
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

/// Update the stored counter for the credential that just authenticated, per
/// the spec's cloned-credential check. We load the matching `Passkey`, ask the
/// library to fold the authentication result into it (`update_credential`
/// advances the counter internally), and re-persist. Pure-server counter
/// inspection isn't possible — `Passkey.cred.counter` is `pub(crate)` — so the
/// library's own mutator is the only safe handle.
///
/// A counter that didn't advance (cloned-credential signal) is logged but not
/// auto-invalidated: at personal scale we surface it to the human via logs
/// rather than lock the user out automatically.
async fn update_passkey_counter(
    db: &Db,
    user_id: &str,
    result: &webauthn_rs::prelude::AuthenticationResult,
) -> Result<()> {
    let passkeys = load_passkeys(db, user_id).await?;
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
    // No matching credential id — shouldn't happen since finish verified it,
    // but treat as a benign no-op rather than failing a verified login.
    tracing::warn!("authenticated credential not found in passkey store");
    Ok(())
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
