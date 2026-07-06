//! SQLite access. One connection guarded by a mutex, touched only inside
//! `spawn_blocking` so the async runtime never blocks on a synchronous query.
//!
//! Single-connection is intentional at personal scale: it makes the
//! extraction + embedding + identity-resolution transaction (ADR-0001) a
//! single `BEGIN … COMMIT` against the in-process `sqlite-vec`, which a
//! multi-connection or external-vector-server setup would break.

use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::auto_extension::RawAutoExtension;
use rusqlite::{params, Connection};

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
    /// This is the internal connection-access mechanism for the
    /// [`SqliteGraphRepo`](crate::graph_repo::SqliteGraphRepo) adapter, the
    /// auth modules, and the health check — NOT for domain modules, which go
    /// through the [`GraphRepo`](crate::graph_repo::GraphRepo) trait.
    ///
    /// The closure owns everything it needs (it must be `'static`); borrow
    /// nothing from the caller — clone owned data into the closure instead.
    pub(crate) async fn with_conn<F, T>(&self, f: F) -> Result<T>
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

    /// Raw connection access for test setup/assertion (issue #48). Production
    /// code uses [`with_conn`](Self::with_conn); domain modules use the
    /// [`GraphRepo`](crate::graph_repo::GraphRepo) trait. Available under
    /// `cfg(test)` and the `test-support` feature so integration-test crates
    /// under `backend/tests/` can run raw SQL for seeding and assertions.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn with_conn_test<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.with_conn(f).await
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

/// The stable id of the bootstrap admin — the single account the deploy-time
/// singleton lock (issue #2) mints on first passkey registration. Issue #72
/// migrates this from a hardcoded constant in `auth::webauthn` into a real row
/// on the `users` table, so every graph row can carry a non-null `user_id` FK
/// to it. The literal is arbitrary but must be constant across restarts so
/// passkeys and graph rows stay associated with the same account.
pub const BOOTSTRAP_ADMIN_USER_ID: &str = "00000000-0000-0000-0000-000000000001";

/// Add a column to `table` if it does not already exist. SQLite has no
/// `ALTER TABLE … ADD COLUMN IF NOT EXISTS`, so we introspect
/// `pragma_table_info` first. Forward-only: once added, the column stays.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    type_def: &str,
) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
        params![table, column],
        |r| r.get(0),
    )?;
    if exists == 0 {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {type_def}"),
            [],
        )?;
    }
    Ok(())
}

/// Whether a vec0 virtual table has a `user_id` partition-key column (issue #72).
/// Used by [`ensure_vec_tables`] to decide whether the pre-issue-72 vec0
/// collections need dropping + recreating with the partition key.
fn vec_table_has_user_id(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = 'user_id'",
        params![table],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Idempotent schema migrations. Each slice appends its `CREATE TABLE IF NOT
/// EXISTS` block; forward-only additive — no destructive ALTERs that lose data.
/// Issue #72 threads a non-null `user_id` FK through every graph table so the
/// knowledge graph is multi-user-capable: each user gets their own Braindumps,
/// Concepts, Edges, Provenance, and inference proposals, isolated from every
/// other user. The existing single-user account (the [`BOOTSTRAP_ADMIN_USER_ID`]
/// constant) is migrated into a real admin row on the `users` table — the
/// bootstrap admin — so no existing data is orphaned.
fn migrate(conn: &Connection) -> Result<()> {
    // Issue #72 — the `users` table. `passkeys.user_id` and `sessions.user_id`
    // reference it (forward-only: the FK is added on fresh DBs; existing DBs
    // keep the TEXT column and rely on the matching row). The bootstrap admin
    // row is inserted idempotently so the existing single-user account maps to
    // a real `users` row.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS users (
            id           TEXT PRIMARY KEY,
            display_name TEXT NOT NULL,
            is_admin     INTEGER NOT NULL DEFAULT 0,
            created_at   INTEGER NOT NULL
        );

        INSERT OR IGNORE INTO users (id, display_name, is_admin, created_at)
        VALUES ('00000000-0000-0000-0000-000000000001', 'me', 1, unixepoch());",
    )?;

    // Issue #2 — passkey auth + opaque sessions. The `user_id` columns now
    // reference `users(id)` (fresh DBs get the FK; existing DBs keep the TEXT
    // column — forward-only).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS passkeys (
            cred_id      BLOB PRIMARY KEY,
            user_id      TEXT NOT NULL REFERENCES users(id),
            passkey_json TEXT NOT NULL,
            created_at   INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            session_id  TEXT PRIMARY KEY,
            user_id     TEXT NOT NULL REFERENCES users(id),
            created_at  INTEGER NOT NULL,
            expires_at  INTEGER
        );

        CREATE INDEX IF NOT EXISTS sessions_user_idx ON sessions(user_id);",
    )?;

    // Issue #72 — scope every graph table by `user_id`. The `users` table and
    // bootstrap admin row already exist (above), so the backfill
    // `UPDATE … SET user_id = BOOTSTRAP_ADMIN_USER_ID WHERE user_id IS NULL`
    // assigns the existing single-user data to the admin. Fresh DBs create
    // the column non-null directly; existing DBs get a nullable column that is
    // backfilled immediately (a later slice may enforce non-null).
    //
    // Issue #5 — braindump ingest skeleton (ADR-0007). A braindump is an
    // immutable thought-snapshot: verbatim (user-confirmed text at submit,
    // overwritable only for error-correction), cleaned (LLM-produced rendering
    // shown by default), and created_at (the original submit instant — edits
    // overwrite in place but never bump the timestamp).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS braindumps (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id     TEXT NOT NULL REFERENCES users(id),
            verbatim    TEXT NOT NULL,
            cleaned     TEXT NOT NULL,
            created_at  INTEGER NOT NULL
        );",
    )?;
    add_column_if_missing(conn, "braindumps", "user_id", "TEXT")?;
    backfill_user_id(conn, "braindumps")?;

    // Issue #3 + #72 — the governed edge-type vocabulary (ontology), now
    // per-user. Fresh DBs create the table with `user_id` and a
    // `(user_id, slug)` unique constraint (replacing the old `slug`-only
    // UNIQUE). Existing DBs are migrated: a new table is created, data is
    // copied with the bootstrap admin's user_id, the old table is dropped,
    // and the new one renamed. The day-zero seed runs per-user on first
    // activity (see `seed_ontology_for_user`); the migration seeds the
    // bootstrap admin so existing data is consistent.
    migrate_ontology_to_per_user(conn)?;

    // Issue #6 + #72 — concept identity, edge accretion, provenance, type
    // history (ADR-0001 / ADR-0002 / ADR-0003 / ADR-0010), now scoped by
    // `user_id`.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS concepts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id     TEXT NOT NULL REFERENCES users(id),
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
            user_id           TEXT NOT NULL REFERENCES users(id),
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
            user_id             TEXT NOT NULL REFERENCES users(id),
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
    for table in ["concepts", "edges", "merge_suggestions"] {
        add_column_if_missing(conn, table, "user_id", "TEXT")?;
        backfill_user_id(conn, table)?;
    }

    // Issue #9 + #72 — ontology governance (ADR-0003), now per-user. The
    // `type_proposals` queue is scoped by `user_id` so each user evolves their
    // own vocabulary.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS type_proposals (
            id                      INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id                 TEXT NOT NULL REFERENCES users(id),
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
    add_column_if_missing(conn, "type_proposals", "user_id", "TEXT")?;
    backfill_user_id(conn, "type_proposals")?;

    // Issue #28 + #72 — delta sync tombstones, now scoped by `user_id`.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS graph_tombstones (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id     TEXT NOT NULL REFERENCES users(id),
            kind        TEXT NOT NULL,
            entity_id   INTEGER NOT NULL,
            created_at  INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS graph_tombstones_kind_created_at_idx
            ON graph_tombstones(kind, created_at);",
    )?;
    add_column_if_missing(conn, "graph_tombstones", "user_id", "TEXT")?;
    backfill_user_id(conn, "graph_tombstones")?;

    // Issue #13 + #72 — thematic-inference snapshots, now scoped by `user_id`.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS thematic_snapshots (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id       TEXT NOT NULL REFERENCES users(id),
            braindump_ids TEXT NOT NULL,
            concept_ids   TEXT NOT NULL,
            captured_at   INTEGER NOT NULL
        );",
    )?;
    add_column_if_missing(conn, "thematic_snapshots", "user_id", "TEXT")?;
    backfill_user_id(conn, "thematic_snapshots")?;

    // Issue #11 + #72 — chat write-back (ADR-0006), now scoped by `user_id`.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS chat_inference_proposals (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id           TEXT NOT NULL REFERENCES users(id),
            mode              TEXT NOT NULL,
            source_concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            target_concept_id INTEGER NOT NULL REFERENCES concepts(id) ON DELETE CASCADE,
            proposed_type     TEXT NOT NULL,
            evidence_path     TEXT NOT NULL,
            rationale         TEXT,
            snapshot_id       INTEGER REFERENCES thematic_snapshots(id),
            status            TEXT NOT NULL DEFAULT 'pending',
            created_at        INTEGER NOT NULL,
            resolved_at       INTEGER
        );

        CREATE INDEX IF NOT EXISTS chat_inference_proposals_status_idx
            ON chat_inference_proposals(status);

        CREATE TABLE IF NOT EXISTS edge_inference_provenance (
            edge_id           INTEGER NOT NULL REFERENCES edges(id) ON DELETE CASCADE,
            chat_inference_id INTEGER NOT NULL REFERENCES chat_inference_proposals(id) ON DELETE CASCADE,
            mode              TEXT NOT NULL,
            snapshot_id       INTEGER REFERENCES thematic_snapshots(id),
            created_at        INTEGER NOT NULL,
            PRIMARY KEY (edge_id, chat_inference_id)
        );",
    )?;
    add_column_if_missing(conn, "chat_inference_proposals", "user_id", "TEXT")?;
    backfill_user_id(conn, "chat_inference_proposals")?;

    Ok(())
}

/// Backfill `user_id` on `table` with the bootstrap admin's id where it is
/// NULL. Forward-only: assigns the existing single-user data to the admin so
/// no row is orphaned. Idempotent — rows already carrying a `user_id` are
/// untouched.
fn backfill_user_id(conn: &Connection, table: &str) -> Result<()> {
    conn.execute(
        &format!("UPDATE {table} SET user_id = ?1 WHERE user_id IS NULL"),
        params![BOOTSTRAP_ADMIN_USER_ID],
    )?;
    Ok(())
}

/// Migrate the `ontology` table from the pre-issue-72 single-shared-vocabulary
/// schema (`slug UNIQUE`) to the per-user schema (`(user_id, slug) UNIQUE`).
///
/// On a fresh DB the table does not exist yet, so this function creates it
/// with the per-user schema and seeds the bootstrap admin's day-zero
/// vocabulary. On an existing DB the old table has `slug UNIQUE` and no
/// `user_id`; this function creates a new table with the per-user schema,
/// copies the old rows with the bootstrap admin's `user_id`, drops the old
/// table, and renames the new one. No data is lost.
fn migrate_ontology_to_per_user(conn: &Connection) -> Result<()> {
    // Check whether the `ontology` table exists at all.
    let table_exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = 'ontology'",
        [],
        |r| r.get(0),
    )?;

    if table_exists == 0 {
        // Fresh DB: create the table with the per-user schema and seed the
        // bootstrap admin's day-zero vocabulary.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS ontology (
                id           INTEGER PRIMARY KEY,
                user_id      TEXT NOT NULL REFERENCES users(id),
                slug         TEXT NOT NULL,
                label        TEXT NOT NULL,
                description  TEXT NOT NULL,
                created_at   INTEGER NOT NULL,
                UNIQUE (user_id, slug)
            );",
        )?;
        seed_ontology_for_user_conn(conn, BOOTSTRAP_ADMIN_USER_ID)?;
        return Ok(());
    }

    // The table exists. Check whether it already has a `user_id` column.
    let has_user_id: i64 = conn.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('ontology') WHERE name = 'user_id'",
        [],
        |r| r.get(0),
    )?;
    if has_user_id > 0 {
        // Already migrated: seed the bootstrap admin idempotently (no-op if
        // already seeded).
        seed_ontology_for_user_conn(conn, BOOTSTRAP_ADMIN_USER_ID)?;
        return Ok(());
    }

    // Existing DB with the old schema: recreate the table with the per-user
    // schema, preserving all data. Standard SQLite migration pattern: create
    // new, copy, drop old, rename.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS ontology_new (
            id           INTEGER PRIMARY KEY,
            user_id      TEXT NOT NULL REFERENCES users(id),
            slug         TEXT NOT NULL,
            label        TEXT NOT NULL,
            description  TEXT NOT NULL,
            created_at   INTEGER NOT NULL,
            UNIQUE (user_id, slug)
        );

        INSERT INTO ontology_new (id, user_id, slug, label, description, created_at)
        SELECT id, '00000000-0000-0000-0000-000000000001', slug, label, description, created_at
        FROM ontology;

        DROP TABLE ontology;

        ALTER TABLE ontology_new RENAME TO ontology;",
    )?;
    Ok(())
}

/// Seed the day-zero edge-type vocabulary for `user_id` (idempotent
/// `INSERT OR IGNORE` scoped to that user). Issue #72: every user starts from
/// the same governed vocabulary and may evolve their own thereafter. Called
/// from the migration for the bootstrap admin and from
/// `seed_ontology_for_user` for new users on first activity.
pub(crate) fn seed_ontology_for_user_conn(conn: &Connection, user_id: &str) -> Result<()> {
    let user_id = user_id.to_string();
    conn.execute(
        "INSERT OR IGNORE INTO ontology (user_id, slug, label, description, created_at) VALUES
        (?1, 'relates_to', 'Relates to', 'Generic association between two concepts; the fallback when no more specific type fits.', unixepoch()),
        (?1, 'causes', 'Causes', 'A brings about B; B would not have occurred without A.', unixepoch()),
        (?1, 'affects', 'Affects', 'A influences or has an effect on B, without strictly causing it.', unixepoch()),
        (?1, 'endangers', 'Endangers', 'A puts B at risk or under threat.', unixepoch()),
        (?1, 'helps', 'Helps', 'A benefits, aids, or contributes positively to B.', unixepoch()),
        (?1, 'part_of', 'Part of', 'A is a component or member of the larger whole B.', unixepoch()),
        (?1, 'depends_on', 'Depends on', 'A requires or relies on B to exist or function.', unixepoch()),
        (?1, 'supports', 'Supports', 'A backs, justifies, or lends weight to B.', unixepoch()),
        (?1, 'contradicts', 'Contradicts', 'A is in tension with or opposes B.', unixepoch()),
        (?1, 'precedes', 'Precedes', 'A comes before B in time or sequence.', unixepoch()),
        (?1, 'enables', 'Enables', 'A makes B possible or allows it to happen.', unixepoch()),
        (?1, 'produces', 'Produces', 'A generates, creates, or yields B.', unixepoch()),
        (?1, 'derived_from', 'Derived from', 'A originates from or is abstracted out of B.', unixepoch())",
        params![user_id],
    )?;
    Ok(())
}

/// Seed the day-zero edge-type vocabulary for `user_id` if it has not been
/// seeded yet. Issue #72: each user starts from the same governed vocabulary
/// on first activity and evolves their own thereafter. Called by the domain
/// layer (ingest, retrieval, ontology reads) on first access for a user.
pub async fn seed_ontology_for_user(db: &Db, user_id: &str) -> Result<()> {
    let user_id = user_id.to_string();
    db.with_conn(move |conn| {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ontology WHERE user_id = ?1",
            params![user_id],
            |r| r.get(0),
        )?;
        if count == 0 {
            seed_ontology_for_user_conn(conn, &user_id)?;
        }
        Ok(())
    })
    .await
}

/// Create the embedding vec0 virtual tables at the given dimensionality.
/// Idempotent. Issue #72: the collections are partitioned by `user_id` so KNN
/// is scoped per-user by construction — a Retrieval KNN seed for one user
/// never returns another user's concepts/braindumps (ADR-0004). If pre-issue-72
/// vec0 collections exist (without the `user_id` partition key), they are
/// dropped and recreated with the partition key; the bootstrap admin's
/// embeddings are re-derived at startup via the type/concept/braindump
/// embedding seed (same shape as `seed_type_embeddings`).
///
/// A model swap (different dim) requires a migration that drops and recreates
/// these tables — out of scope for this slice.
pub(crate) fn ensure_vec_tables(conn: &Connection, dim: usize) -> Result<()> {
    assert!(dim > 0, "embedding dimension must be positive");

    // If pre-issue-72 vec0 collections exist (no `user_id` partition key),
    // drop them so the `CREATE … IF NOT EXISTS` below recreates them with the
    // partition key. The embeddings are re-derived at startup (issue #72
    // backfill); no source data is lost — the braindumps/concepts/ontology
    // rows persist, only the derived vectors are recomputed.
    for table in [
        "concept_embeddings",
        "braindump_embeddings",
        "type_embeddings",
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                params![table],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if exists > 0 && !vec_table_has_user_id(conn, table) {
            conn.execute(&format!("DROP TABLE {table}"), [])?;
        }
    }

    let ddl = format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS concept_embeddings USING vec0(
            concept_id INTEGER PRIMARY KEY,
            user_id TEXT PARTITION KEY,
            embedding float[{dim}] distance_metric=cosine
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS braindump_embeddings USING vec0(
            braindump_id INTEGER PRIMARY KEY,
            user_id TEXT PARTITION KEY,
            embedding float[{dim}] distance_metric=cosine
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS type_embeddings USING vec0(
            ontology_id INTEGER PRIMARY KEY,
            user_id TEXT PARTITION KEY,
            embedding float[{dim}] distance_metric=cosine
        );"
    );
    conn.execute_batch(&ddl)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;

    #[tokio::test]
    async fn migrate_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        // Opening already migrated; migrating again should not error.
        db.with_conn(|conn| migrate(conn).map(|_| ()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn migrations_create_expected_tables() {
        let db = Db::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            assert!(names.contains(&"users".to_string()));
            assert!(names.contains(&"passkeys".to_string()));
            assert!(names.contains(&"sessions".to_string()));
            assert!(names.contains(&"ontology".to_string()));
            Ok(())
        })
        .await
        .unwrap();
    }

    /// Issue #72: the bootstrap admin row must exist on the `users` table so
    /// the existing single-user account maps to a real `users` row.
    #[tokio::test]
    async fn bootstrap_admin_row_exists() {
        let db = Db::open_in_memory().unwrap();
        db.with_conn(|conn| {
            let row: (String, String, i64) = conn.query_row(
                "SELECT id, display_name, is_admin FROM users WHERE id = ?1",
                params![BOOTSTRAP_ADMIN_USER_ID],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            assert_eq!(row.0, BOOTSTRAP_ADMIN_USER_ID);
            assert_eq!(row.1, "me");
            assert_eq!(row.2, 1, "bootstrap admin must be is_admin=1");
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
        db.with_conn(|conn| {
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
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT COUNT(*) FROM ontology", [], |r| r.get::<_, i64>(0))?)
            })
            .await
            .unwrap();
        db.with_conn(|conn| migrate(conn).map(|_| ()))
            .await
            .unwrap();
        let count_second = db
            .with_conn(|conn| {
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
        db.with_conn(|conn| {
            let mut stmt =
                conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")?;
            let names: Vec<String> = stmt
                .query_map([], |r| r.get::<_, String>(0))?
                .filter_map(std::result::Result::ok)
                .collect();
            for expected in [
                "users",
                "concepts",
                "concept_provenance",
                "edges",
                "edge_provenance",
                "edge_type_history",
                "merge_suggestions",
                "type_proposals",
                "graph_tombstones",
                "chat_inference_proposals",
                "edge_inference_provenance",
                "thematic_snapshots",
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
        db.with_conn(|conn| {
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
            // Issue #72: each vec0 collection must have a `user_id` partition
            // key column so KNN is scoped per-user by construction.
            for table in [
                "concept_embeddings",
                "braindump_embeddings",
                "type_embeddings",
            ] {
                assert!(
                    vec_table_has_user_id(conn, table),
                    "vec0 table `{table}` must have a user_id partition key"
                );
            }
            Ok(())
        })
        .await
        .unwrap();
    }

    /// Issue #72: `seed_ontology_for_user` seeds the day-zero vocabulary for a
    /// new user idempotently. Each user starts from the same governed
    /// vocabulary and evolves their own thereafter.
    #[tokio::test]
    async fn seed_ontology_for_user_seeds_and_is_idempotent() {
        let db = Db::open_in_memory().unwrap();
        let user = "00000000-0000-0000-0000-000000000002".to_string();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO users (id, display_name, is_admin, created_at)
                 VALUES (?1, 'user2', 0, unixepoch())",
                params![user],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        seed_ontology_for_user(&db, "00000000-0000-0000-0000-000000000002")
            .await
            .unwrap();
        let u = "00000000-0000-0000-0000-000000000002".to_string();
        let count_first = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM ontology WHERE user_id = ?1",
                    params![u],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(
            count_first,
            EXPECTED_SEED_SLUGS.len() as i64,
            "new user must get the full day-zero vocabulary"
        );

        // Idempotent: seeding again does not duplicate.
        seed_ontology_for_user(&db, "00000000-0000-0000-0000-000000000002")
            .await
            .unwrap();
        let u = "00000000-0000-0000-0000-000000000002".to_string();
        let count_second = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM ontology WHERE user_id = ?1",
                    params![u],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(count_first, count_second, "re-seeding must not duplicate");

        // A second user gets their own independent vocabulary.
        let user_b = "00000000-0000-0000-0000-000000000003".to_string();
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT INTO users (id, display_name, is_admin, created_at)
                 VALUES (?1, 'user3', 0, unixepoch())",
                params![user_b],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        seed_ontology_for_user(&db, "00000000-0000-0000-0000-000000000003")
            .await
            .unwrap();
        let ub = "00000000-0000-0000-0000-000000000003".to_string();
        let count_b = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT COUNT(*) FROM ontology WHERE user_id = ?1",
                    params![ub],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(count_b, count_first, "second user gets the same vocabulary");
    }
}
