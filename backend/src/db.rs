//! SQLite access. One connection guarded by a mutex, touched only inside
//! `spawn_blocking` so the async runtime never blocks on a synchronous query.
//!
//! Single-connection is intentional at personal scale: it makes the
//! extraction + embedding + identity-resolution transaction (ADR-0001) a
//! single `BEGIN … COMMIT` against the in-process `sqlite-vec`, which a
//! multi-connection or external-vector-server setup would break.

use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::auto_extension::RawAutoExtension;
use rusqlite::Connection;

use crate::error::{Error, Result};

#[derive(Clone)]
pub struct Db(Arc<Mutex<Connection>>);

/// Register the `sqlite-vec` extension as a process-global auto-extension so
/// every connection opened after this call has it available. Idempotent: the
/// registration runs once and its outcome is cached.
fn ensure_sqlite_vec() -> Result<()> {
    static REGISTERED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    let result = REGISTERED.get_or_init(|| {
        // SAFETY: `sqlite3_vec_init` is the sqlite-vec init entry point; the
        // sqlite-vec crate declares it with an arg-less signature, so we take
        // its address and transmute to the auto-extension callback type.
        // SQLite then calls it with the correct `(db, errmsg, api)` args. The
        // extension is read-only and thread-safe.
        let init: RawAutoExtension =
            unsafe { std::mem::transmute(sqlite_vec::sqlite3_vec_init as *const ()) };
        unsafe { rusqlite::auto_extension::register_auto_extension(init) }
            .map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => Ok(()),
        Err(msg) => Err(Error::SqliteVec(msg.clone())),
    }
}

impl Db {
    /// Open a connection at `path` (`:memory:` for an in-memory database),
    /// apply pragmatic PRAGMAs, load the `sqlite-vec` extension, and run
    /// idempotent schema migrations.
    pub fn open(path: &str) -> Result<Self> {
        ensure_sqlite_vec()?;
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
        migrate(&conn)?;
        Ok(Self(Arc::new(Mutex::new(conn))))
    }

    /// Convenience for tests and ephemeral instances.
    pub fn open_in_memory() -> Result<Self> {
        Self::open(":memory:")
    }

    /// Run `f` against the connection on a blocking thread.
    ///
    /// The closure owns everything it needs (it must be `'static`); borrow
    /// nothing from the caller — clone owned data into the closure instead.
    pub async fn run<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let conn = inner.lock().expect("db mutex poisoned");
            f(&conn)
        })
        .await
        .map_err(|e| Error::Internal(e.to_string()))?
    }
}

/// Wall-clock seconds since the Unix epoch. Used for `created_at` / `expires_at`
/// columns so timestamps survive process restarts as plain integers.
pub fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Idempotent schema migrations. Each slice appends its `CREATE TABLE IF NOT
/// EXISTS` block; no destructive ALTERs — this is a personal-scale, single-connection
/// app where forward-only additive migrations suffice.
fn migrate(conn: &Connection) -> Result<()> {
    // Issue #2 — passkey auth + opaque sessions.
    //
    // `passkeys` holds the registered WebAuthn credentials (one row per
    // credential id). The `Passkey` value is JSON-serialisable so we store it
    // verbatim and reconstruct it for authentication.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS passkeys (
            cred_id      BLOB PRIMARY KEY,
            user_id      TEXT NOT NULL,
            passkey_json TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            session_id  TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            expires_at   INTEGER
        );

        CREATE INDEX IF NOT EXISTS sessions_user_idx ON sessions(user_id);",
    )?;

    // Issue #5 — braindump ingest skeleton (ADR-0007). A braindump is an
    // immutable thought-snapshot: verbatim (user-confirmed text at submit,
    // overwritable only for error-correction), cleaned (LLM-produced rendering
    // shown by default), and created_at (the original submit instant — edits
    // overwrite in place but never bump the timestamp). Extraction is stubbed
    // in this slice; concepts/edges tables land with the real extraction slice.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS braindumps (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            verbatim    TEXT NOT NULL,
            cleaned     TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        // Opening already migrated; migrating again should not error.
        db.run(|conn| migrate(conn).map(|_| ())).await.unwrap();
    }

    #[tokio::test]
    async fn migrations_create_expected_tables() {
        let db = Db::open_in_memory().unwrap();
        db.run(|conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            assert!(names.contains(&"passkeys".to_string()));
            assert!(names.contains(&"sessions".to_string()));
            Ok(())
        })
        .await
        .unwrap();
    }
}
