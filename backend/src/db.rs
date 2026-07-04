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

    /// Create the `sqlite-vec` vec0 virtual tables for concept and braindump
    /// embeddings. The dimensionality is fixed by the configured embedding
    /// model, so this is a one-time setup call run at `AppState` construction
    /// (not in `migrate`, which is dim-agnostic). Sync + brief: a one-shot
    /// `CREATE VIRTUAL TABLE IF NOT EXISTS` at startup; the async runtime is
    /// not blocked meaningfully.
    ///
    /// `vec0(... distance_metric=cosine)` makes the `MATCH ... ORDER BY
    /// distance` KNN query return cosine distance (1 − cosine similarity), so
    /// identity resolution reads similarity = 1 − distance (ADR-0001).
    pub fn ensure_vec_tables(&self, dim: usize) -> Result<()> {
        let conn = self.0.lock().expect("db mutex poisoned");
        ensure_vec_tables(&conn, dim)
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

    // Issue #3 — the governed edge-type vocabulary (ontology). The LLM draws
    // types from here and never invents beyond it; governance (propose/approve,
    // type-embeddings, event-sourced refactor — ADR-0003) is a later slice.
    //
    // `id` is a stable surrogate primary key. The future type-embeddings vec
    // table and the per-edge event-sourced type-history log both FK-reference
    // `ontology.id`, so those slices append new tables and never re-migrate
    // this one. `slug` is the machine key the LLM emits; governance may rename
    // a slug via a refactor (recorded in the history log), but the `id` it
    // anchors on is immutable. Seeds use `INSERT OR IGNORE` so re-opening a
    // database is idempotent and never duplicates the day-zero vocabulary.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ontology (
            id           INTEGER PRIMARY KEY,
            slug         TEXT NOT NULL UNIQUE,
            label        TEXT NOT NULL,
            description  TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );

        INSERT OR IGNORE INTO ontology (slug, label, description, created_at) VALUES
        ('relates_to', 'Relates to', 'Generic association between two concepts; the fallback when no more specific type fits.', unixepoch()),
        ('causes', 'Causes', 'A brings about B; B would not have occurred without A.', unixepoch()),
        ('affects', 'Affects', 'A influences or has an effect on B, without strictly causing it.', unixepoch()),
        ('endangers', 'Endangers', 'A puts B at risk or under threat.', unixepoch()),
        ('helps', 'Helps', 'A benefits, aids, or contributes positively to B.', unixepoch()),
        ('part_of', 'Part of', 'A is a component or member of the larger whole B.', unixepoch()),
        ('depends_on', 'Depends on', 'A requires or relies on B to exist or function.', unixepoch()),
        ('supports', 'Supports', 'A backs, justifies, or lends weight to B.', unixepoch()),
        ('contradicts', 'Contradicts', 'A is in tension with or opposes B.', unixepoch()),
        ('precedes', 'Precedes', 'A comes before B in time or sequence.', unixepoch()),
        ('enables', 'Enables', 'A makes B possible or allows it to happen.', unixepoch()),
        ('produces', 'Produces', 'A generates, creates, or yields B.', unixepoch()),
        ('derived_from', 'Derived from', 'A originates from or is abstracted out of B.', unixepoch());",
    )?;

    // Issue #6 — concept identity, edge accretion, provenance, type history
    // (ADR-0001 / ADR-0002 / ADR-0003 / ADR-0010). Forward-only additive: new
    // tables only, the existing ontology/braindumps tables are untouched.
    //
    // Concepts accrete by embedding match (ADR-0001); identity is not label
    // equality. `concept_provenance` is the extraction provenance symmetric to
    // edge provenance (ADR-0010): deleting a braindump drops its row here, and a
    // concept vanishes when its last extractor is removed.
    //
    // Edges accrete by (source, original_type, target) — the original_type is
    // the LLM's first assertion and anchors identity; the *current* type is a
    // projection off `edge_type_history` (ADR-0003), never a stored field.
    // `edge_provenance` is the asserted_by list (ADR-0002). Type-history rows
    // cascade on edge deletion; provenance rows cascade on concept/edge/braindump
    // deletion.
    //
    // `merge_suggestions` surfaces borderline identity pairs for human
    // confirm/reject (ADR-0001); the queue/approval UI is a later slice.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS concepts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            label       TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS concept_provenance (
            concept_id    INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            braindump_id  INTEGER NOT NULL REFERENCES braindumps(id) ON DELETE CASCADE,
            PRIMARY KEY (concept_id, braindump_id)
        );

        CREATE TABLE IF NOT EXISTS edges (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            source_concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            target_concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            original_type     TEXT NOT NULL,
            created_at        INTEGER NOT NULL,
            UNIQUE (source_concept_id, original_type, target_concept_id)
        );

        CREATE TABLE IF NOT EXISTS edge_provenance (
            edge_id      INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
            braindump_id INTEGER NOT NULL REFERENCES braindumps(id) ON DELETE CASCADE,
            PRIMARY KEY (edge_id, braindump_id)
        );

        CREATE TABLE IF NOT EXISTS edge_type_history (
            edge_id    INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
            seq_index  INTEGER NOT NULL,
            type_slug  TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY (edge_id, seq_index)
        );

        CREATE TABLE IF NOT EXISTS merge_suggestions (
            id                  INTEGER PRIMARY KEY AUTOINCREMENT,
            kind                TEXT NOT NULL,
            braindump_id        INTEGER NOT NULL REFERENCES braindumps(id) ON DELETE CASCADE,
            new_concept_label   TEXT NOT NULL,
            new_concept_id      INTEGER REFERENCES concepts(id) ON DELETE CASCADE,
            existing_concept_id INTEGER REFERENCES concepts(id) ON DELETE CASCADE,
            similarity          REAL NOT NULL,
            status              TEXT NOT NULL DEFAULT 'pending',
            created_at          INTEGER NOT NULL
        );",
    )?;

    // Issue #9 — ontology governance (ADR-0003). Forward-only additive: a new
    // `type_proposals` table for the propose/approve/reject queue + dedup
    // metadata. The type-embeddings vec0 collection is created in
    // `ensure_vec_tables` (it is dim-dependent, like the concept/braindump
    // collections). The existing `ontology` table is NOT re-migrated.
    //
    // `merge_of` references an ontology slug (not id) the proposed type
    // replaces; on approve, the refactor retags edges whose current type is
    // `merge_of` to the new slug. `near_match_*` records the dedup decision
    // for the human reviewer (auto-merged above 99.5%, else pending).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS type_proposals (
            id                      INTEGER PRIMARY KEY AUTOINCREMENT,
            slug                    TEXT NOT NULL,
            label                   TEXT NOT NULL,
            description             TEXT NOT NULL,
            merge_of                TEXT,
            status                  TEXT NOT NULL DEFAULT 'pending',
            near_match_slug         TEXT,
             near_match_similarity   REAL,
             created_at              INTEGER NOT NULL,
             resolved_at             INTEGER
         );",
    )?;

    // Issue #28 — delta sync (pull-on-focus reconciliation). Forward-only
    // additive: a new `graph_tombstones` append-only log records concepts/edges
    // that vanished via the deletion cascade (ADR-0007/0010). The cascade
    // deletes rows outright, so without a tombstone log a delta-since-timestamp
    // read could report additions and retags (both already carry `created_at`)
    // but not what disappeared. Existing tables are NOT re-migrated; the
    // tombstone log is written from the cascade in `graph::retract_extraction`
    // and read by `delta::graph_delta`.
    //
    // `kind` discriminates 'concept' vs 'edge'; `entity_id` is the vanished
    // row's surrogate id (kept as a plain integer — the row is gone, so no FK).
    // Append-only: nothing ever DELETEs from here.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS graph_tombstones (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            kind        TEXT NOT NULL,
            entity_id   INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS graph_tombstones_kind_created_at_idx
            ON graph_tombstones(kind, created_at);",
    )?;
    Ok(())
}

/// Create the embedding vec0 virtual tables at the given dimensionality.
/// Idempotent. A model swap (different dim) requires a migration that drops
/// and recreates these tables — out of scope for this slice.
pub(crate) fn ensure_vec_tables(conn: &Connection, dim: usize) -> Result<()> {
    assert!(dim > 0, "embedding dimension must be positive");
    let ddl = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS concept_embeddings USING vec0(
            concept_id INTEGER PRIMARY KEY,
            embedding float[{dim}] distance_metric=cosine
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS braindump_embeddings USING vec0(
            braindump_id INTEGER PRIMARY KEY,
            embedding float[{dim}] distance_metric=cosine
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS type_embeddings USING vec0(
            ontology_id INTEGER PRIMARY KEY,
            embedding float[{dim}] distance_metric=cosine
        );"
    );
    conn.execute_batch(&ddl)?;
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
            assert!(names.contains(&"ontology".to_string()));
            Ok(())
        })
        .await
        .unwrap();
    }

    /// The day-zero edge-type vocabulary the LLM draws from. Independent of the
    /// seed implementation: the expected slugs are a known-good literal sourced
    /// from the issue spec, not recomputed from the migration.
    const EXPECTED_SEED_SLUGS: &[&str] = &[
        "relates_to",
        "causes",
        "affects",
        "endangers",
        "helps",
        "part_of",
        "depends_on",
        "supports",
        "contradicts",
        "precedes",
        "enables",
        "produces",
        "derived_from",
    ];

    #[tokio::test]
    async fn ontology_table_is_seeded_with_edge_types() {
        let db = Db::open_in_memory().unwrap();
        db.run(|conn| {
            let mut stmt =
                conn.prepare("SELECT slug, label, description FROM ontology ORDER BY id")?;
            let rows: Vec<(String, String, String)> = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            assert!(!rows.is_empty(), "ontology must be seeded with edge types");

            let slugs: Vec<&str> = rows.iter().map(|(s, _, _)| s.as_str()).collect();
            let mut sorted = slugs.to_vec();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(
                slugs.len(),
                sorted.len(),
                "ontology slugs must be unique: {slugs:?}"
            );

            for slug in EXPECTED_SEED_SLUGS {
                assert!(
                    slugs.contains(slug),
                    "ontology missing seed slug `{slug}`: {slugs:?}"
                );
            }
            for (slug, label, description) in &rows {
                assert!(!label.is_empty(), "label for `{slug}` must be non-empty");
                assert!(
                    !description.is_empty(),
                    "description for `{slug}` must be non-empty"
                );
            }

            let causes = rows
                .iter()
                .find(|(s, _, _)| s == "causes")
                .expect("causes is seeded");
            assert_eq!(causes.1, "Causes");
            assert_eq!(
                causes.2,
                "A brings about B; B would not have occurred without A."
            );
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn ontology_seed_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let count_first = db
            .run(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM ontology", [], |r| r.get::<_, i64>(0))?)
            })
            .await
            .unwrap();
        db.run(|conn| migrate(conn).map(|_| ())).await.unwrap();
        let count_second = db
            .run(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM ontology", [], |r| r.get::<_, i64>(0))?)
            })
            .await
            .unwrap();
        assert!(count_first > 0, "ontology must be seeded on first open");
        assert_eq!(
            count_first, count_second,
            "re-migrating must not duplicate seeds"
        );
    }

    #[tokio::test]
    async fn migrations_create_extraction_tables() {
        let db = Db::open_in_memory().unwrap();
        db.run(|conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            for expected in [
                "concepts",
                "concept_provenance",
                "edges",
                "edge_provenance",
                "edge_type_history",
                "merge_suggestions",
                "type_proposals",
                "graph_tombstones",
            ] {
                assert!(
                    names.contains(&expected.to_string()),
                    "expected table `{expected}` must exist: {names:?}"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn ensure_vec_tables_creates_knn_virtual_tables() {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(8).unwrap();
        db.run(|conn| {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master
                 WHERE type='table' AND name IN (?, ?, ?)
                 ORDER BY name",
            )?;
            let names: Vec<String> = stmt
                .query_map(
                    [
                        "concept_embeddings",
                        "braindump_embeddings",
                        "type_embeddings",
                    ],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<_>>()?;
            assert_eq!(
                names,
                [
                    "braindump_embeddings",
                    "concept_embeddings",
                    "type_embeddings"
                ],
                "all three vec0 virtual tables must exist"
            );
            Ok(())
        })
        .await
        .unwrap();
    }
}
