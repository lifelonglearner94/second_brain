//! The graph-repository seam (issue #44).
//!
//! Establishes the trait that every later slice widens. Today it carries one
//! method — whether a braindump's embedding is stored (the smallest possible
//! read) — so migrating it proves the trait shape, the adapter wiring, and the
//! in-memory test pattern all work end-to-end before the larger slices land.
//!
//! `Db` stays as a connection holder; `Db::run` is untouched. The free
//! function `graph::braindump_embedding_stored` remains alongside this seam
//! and is removed in a later slice (#48) — "make the change easy, then make
//! the easy change." Later slices (#45/#46/#47) add methods to this trait.

use async_trait::async_trait;
use rusqlite::params;

use crate::db::Db;
use crate::error::Result;

/// The graph-repository seam. Reads against the knowledge graph behind one
/// trait so call sites depend on the interface, not the storage adapter.
///
/// Production wires [`SqliteGraphRepo`]; tests wire [`InMemoryGraphRepo`] so
/// the braindump-embedding check (and, in later slices, every other read) can
/// be exercised without opening a SQLite connection. The trait starts small —
/// one method — and grows as later slices migrate the remaining reads.
#[async_trait]
pub trait GraphRepo: Send + Sync {
    /// Whether a braindump's embedding is stored (retrieval backfill,
    /// ADR-0004). The smallest possible read; migrating it proves the seam.
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool>;
}

/// Production adapter: delegates to [`Db::run`] against the in-process
/// `sqlite-vec`, so the single-connection transaction guarantees of `Db`
/// (ADR-0001) are preserved. `Db::run` itself is untouched.
pub struct SqliteGraphRepo {
    db: Db,
}

impl SqliteGraphRepo {
    /// Wrap a [`Db`] handle. `Db` is `Clone` (inner `Arc`), so a production
    /// `AppState` and this adapter may share one connection cheaply.
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[async_trait]
impl GraphRepo for SqliteGraphRepo {
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool> {
        self.db
            .run(move |conn| {
                let count: i64 = conn.query_row(
                    "SELECT count(*) FROM braindump_embeddings WHERE braindump_id = ?1",
                    params![braindump_id],
                    |r| r.get(0),
                )?;
                Ok(count > 0)
            })
            .await
    }
}

/// In-memory adapter for tests: holds the set of braindump ids whose embedding
/// is "stored" so the braindump-embedding check can be exercised without a
/// SQLite connection. Gate on `test` and the forward-looking `test-support`
/// feature so integration-test crates (in `backend/tests/`) can enable it.
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryGraphRepo {
    stored: std::sync::Mutex<std::collections::HashSet<i64>>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryGraphRepo {
    pub fn new() -> Self {
        Self {
            stored: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    /// Mark a braindump's embedding as stored so a subsequent
    /// [`GraphRepo::braindump_embedding_stored`] returns `true`.
    pub fn mark_braindump_embedding_stored(&self, braindump_id: i64) {
        self.stored
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(braindump_id);
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for InMemoryGraphRepo {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl GraphRepo for InMemoryGraphRepo {
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool> {
        let stored = self
            .stored
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        Ok(stored.contains(&braindump_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The braindump-embedding check is reachable through the seam without a
    /// SQLite connection: a fresh `InMemoryGraphRepo` reports nothing stored,
    /// and after marking, the id reports stored.
    #[tokio::test]
    async fn in_memory_reports_stored_after_marking() {
        let repo = InMemoryGraphRepo::new();
        assert!(
            !repo.braindump_embedding_stored(42).await.unwrap(),
            "fresh repo reports nothing stored"
        );
        repo.mark_braindump_embedding_stored(42);
        assert!(
            repo.braindump_embedding_stored(42).await.unwrap(),
            "marked id reports stored"
        );
    }
}
