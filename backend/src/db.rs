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
    /// apply pragmatic PRAGMAs, and load the `sqlite-vec` extension.
    pub fn open(path: &str) -> Result<Self> {
        ensure_sqlite_vec()?;
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;
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
