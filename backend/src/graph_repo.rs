//! The graph-repository seam (issues #44 / #45).
//!
//! Every read against the knowledge graph goes through [`GraphRepo`] so call
//! sites depend on the interface, not the storage adapter. Production wires
//! [`SqliteGraphRepo`] (delegating to [`Db::run`](crate::db::Db)); tests wire
//! [`InMemoryGraphRepo`] so any read can be exercised without a SQLite
//! connection — the read model becomes hermetic.
//!
//! After #45 the SQL that defines "the current type is the projected state of
//! the type history" (ADR-0003) and the byte layout sqlite-vec expects both
//! live here — in the Sqlite adapter — instead of being copy-pasted across
//! `graph`, `ontology`, `retrieval`, and `snapshot`. The free-function read
//! helpers in `graph.rs` / `ontology.rs` remain as one-line delegators to this
//! trait so existing callers (including the integration tests under
//! `backend/tests/`) keep compiling; #48 removes them once every caller is
//! migrated to take a `&dyn GraphRepo` directly.

use async_trait::async_trait;
use rusqlite::{params, OptionalExtension};

use crate::db::Db;
use crate::error::Result;
use crate::graph::{Concept, Edge, EdgeProjection, MergeSuggestion, TypeHistoryEntry};

/// The canonical current-type projection SQL fragment (ADR-0003): the last
/// `edge_type_history` entry, correlated on the outer edges alias `e`.
///
/// Lives in the Sqlite adapter's home so the projection lives in one place.
/// `pub(crate)` (not private) so the still-present write-path closures in
/// `graph.rs` / `ontology.rs` / `delta.rs` / `retrieval.rs` that call it inside
/// their own `Db::run` closures keep compiling — #46 puts writes behind the
/// trait and #48 removes the old closures; after both land this becomes truly
/// private.
pub(crate) fn current_type_subquery() -> &'static str {
    "SELECT type_slug FROM edge_type_history WHERE edge_id = e.id ORDER BY seq_index DESC LIMIT 1"
}

/// f32 slice → little-endian byte blob, the on-disk format sqlite-vec expects.
///
/// Lives in the Sqlite adapter's home so the byte layout is defined once.
/// `pub(crate)` (not private) so the still-present write-path closures in
/// `graph.rs` (`create_concept`, `store_braindump_embedding`) and
/// `ontology.rs` (`approve_proposal`, `seed_type_embeddings`) that call it
/// inside their own `Db::run` closures keep compiling — #46 puts writes behind
/// the trait and #48 removes the old closures; after both land this becomes
/// truly private.
pub(crate) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

/// sqlite-vec KNN over `concept_embeddings`: nearest concept by cosine.
/// Returns `(concept_id, similarity)` where similarity = 1 − distance. `None`
/// if the collection is empty.
///
/// Lives in the Sqlite adapter's home as `pub(crate)` so the still-present
/// accretion write-path closure in `graph.rs` (`resolve_concept` →
/// `ingest_extraction`'s `Db::run`) can call it synchronously inside the
/// transaction — the KNN must see the post-retraction state (embeddings for
/// vanished concepts are deleted before identity resolution runs), so it
/// cannot be lifted out of the closure to call the async trait method.
/// #46 puts the write path behind the trait (the accretion KNN call site
/// routes through `GraphRepo::knn_concept` directly) and #48 removes the old
/// closure; after both land this becomes truly private.
pub(crate) fn knn_concept_conn(
    conn: &rusqlite::Connection,
    query_vec: &[f32],
) -> Result<Option<(i64, f32)>> {
    let blob = vec_to_blob(query_vec);
    let row = conn
        .prepare(
            "SELECT concept_id, distance FROM concept_embeddings
             WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
        )?
        .query_row(params![blob], |r| {
            Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
        })
        .optional()?;
    Ok(row)
}

/// The graph-repository seam. Reads against the knowledge graph behind one
/// trait so call sites depend on the interface, not the storage adapter.
///
/// Production wires [`SqliteGraphRepo`]; tests wire [`InMemoryGraphRepo`] so
/// every read can be exercised without opening a SQLite connection. The trait
/// started small (issue #44: the braindump-embedding check) and widened in
/// #45 to cover every scattered read helper — single-row reads, list reads,
/// provenance reads, and the vec0 KNN sites in the graph, ontology, and
/// retrieval paths.
#[async_trait]
pub trait GraphRepo: Send + Sync {
    /// Whether a braindump's embedding is stored (retrieval backfill,
    /// ADR-0004). The smallest possible read; #44 migrated it to prove the
    /// seam.
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool>;

    /// All ontology types as `(slug, label, description)`, ordered by `id`.
    /// The single source of truth for the full-row ontology read — the query
    /// that was duplicated across `ontology.rs` and `routes/ontology.rs` lives
    /// here now (in the Sqlite adapter) and nowhere else.
    async fn ontology_types(&self) -> Result<Vec<(String, String, String)>>;

    /// The governed edge-type slugs the LLM draws from. Derived from
    /// [`ontology_types`](GraphRepo::ontology_types) so the slug-only read and
    /// the full-row read cannot drift apart.
    async fn ontology_slugs(&self) -> Result<Vec<String>> {
        Ok(self
            .ontology_types()
            .await?
            .into_iter()
            .map(|(slug, _, _)| slug)
            .collect())
    }

    /// A single concept by id (read model). Identity is by embedding
    /// (ADR-0001), not id; this is the inspection/helper path. `None` if no
    /// concept with `id` exists.
    async fn get_concept(&self, id: i64) -> Result<Option<Concept>>;

    /// The braindump ids that extracted a concept (ADR-0010 extraction
    /// provenance), ordered by braindump id.
    async fn concept_provenance(&self, concept_id: i64) -> Result<Vec<i64>>;

    /// Look up an edge by its identity key `(source, original_type, target)`
    /// (ADR-0002). `original_type` anchors identity and is immutable; the
    /// current type is a projection off the type history (ADR-0003) and is
    /// NOT returned here — see [`all_edges_with_current_type`] /
    /// [`edge_type_history`](GraphRepo::edge_type_history). `None` if no edge
    /// matches.
    ///
    /// [`all_edges_with_current_type`]: GraphRepo::all_edges_with_current_type
    async fn find_edge(
        &self,
        source_id: i64,
        original_type: &str,
        target_id: i64,
    ) -> Result<Option<Edge>>;

    /// The braindump ids asserting an edge (ADR-0002 `asserted_by`), ordered
    /// by braindump id.
    async fn edge_provenance(&self, edge_id: i64) -> Result<Vec<i64>>;

    /// The append-only type history of an edge (ADR-0003). Index 0 is the
    /// original assertion; the last entry is the current (projected) type.
    async fn edge_type_history(&self, edge_id: i64) -> Result<Vec<TypeHistoryEntry>>;

    /// All pending + resolved merge suggestions (ADR-0001), ordered by id.
    async fn merge_suggestions(&self) -> Result<Vec<MergeSuggestion>>;

    /// Look up a concept id by exact label. Identity is by embedding
    /// (ADR-0001), not label, so this is a test/inspection helper — not the
    /// identity path. `None` if no concept with the label exists.
    async fn concept_id_for_label(&self, label: &str) -> Result<Option<i64>>;

    /// Every concept, ordered by id — the full node set for whole-graph reads
    /// (issue #27's Global Topology Snapshot).
    async fn all_concepts(&self) -> Result<Vec<Concept>>;

    /// Every edge with its projected current type (ADR-0003), ordered by id.
    /// The current type is the last `edge_type_history` entry; for a
    /// freshly-created edge it equals the original assertion.
    async fn all_edges_with_current_type(&self) -> Result<Vec<EdgeProjection>>;

    /// sqlite-vec KNN: nearest concept by cosine. Returns `(concept_id,
    /// similarity)` where similarity = 1 − distance (cosine metric on the vec0
    /// table). `None` if no concepts exist yet. Used by concept identity
    /// resolution / accretion (ADR-0001).
    async fn knn_concept(&self, query_vec: &[f32]) -> Result<Option<(i64, f32)>>;

    /// sqlite-vec KNN: nearest ontology type by cosine. Returns `(slug,
    /// similarity)` where similarity = 1 − distance. `None` if the
    /// type-embedding collection is empty. Used by ontology governance dedup
    /// (ADR-0003).
    async fn knn_type(&self, query_vec: &[f32]) -> Result<Option<(String, f32)>>;

    /// sqlite-vec KNN: top-K concepts by cosine similarity to the query
    /// vector. similarity = 1 − distance. Used by retrieval seed
    /// (ADR-0004).
    async fn knn_concepts(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>>;

    /// sqlite-vec KNN: top-K braindumps by cosine similarity to the query
    /// vector. similarity = 1 − distance. Used by retrieval backfill and the
    /// no-seed fallback (ADR-0004).
    async fn knn_braindumps(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>>;
}

/// Production adapter: delegates to [`Db::run`] against the in-process
/// `sqlite-vec`, so the single-connection transaction guarantees of `Db`
/// (ADR-0001) are preserved. `Db::run` itself is untouched. Owns the SQL bodies
/// for every read so the domain modules (`graph`, `ontology`, `retrieval`,
/// `snapshot`) no longer contain raw read SQL.
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

    async fn ontology_types(&self) -> Result<Vec<(String, String, String)>> {
        self.db
            .run(|conn| {
                let mut stmt =
                    conn.prepare("SELECT slug, label, description FROM ontology ORDER BY id")?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn get_concept(&self, id: i64) -> Result<Option<Concept>> {
        self.db
            .run(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT id, label, created_at FROM concepts WHERE id = ?1",
                        params![id],
                        |r| {
                            Ok(Concept {
                                id: r.get(0)?,
                                label: r.get(1)?,
                                created_at: r.get(2)?,
                            })
                        },
                    )
                    .optional()?;
                Ok(row)
            })
            .await
    }

    async fn concept_provenance(&self, concept_id: i64) -> Result<Vec<i64>> {
        self.db
            .run(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT braindump_id FROM concept_provenance
                     WHERE concept_id = ?1 ORDER BY braindump_id",
                )?;
                let ids = stmt
                    .query_map(params![concept_id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(ids)
            })
            .await
    }

    async fn find_edge(
        &self,
        source_id: i64,
        original_type: &str,
        target_id: i64,
    ) -> Result<Option<Edge>> {
        let original_type = original_type.to_string();
        self.db
            .run(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT id, source_concept_id, target_concept_id, original_type, created_at
                         FROM edges
                         WHERE source_concept_id = ?1 AND original_type = ?2 AND target_concept_id = ?3",
                        params![source_id, original_type, target_id],
                        |r| {
                            Ok(Edge {
                                id: r.get(0)?,
                                source_concept_id: r.get(1)?,
                                target_concept_id: r.get(2)?,
                                original_type: r.get(3)?,
                                created_at: r.get(4)?,
                            })
                        },
                    )
                    .optional()?;
                Ok(row)
            })
            .await
    }

    async fn edge_provenance(&self, edge_id: i64) -> Result<Vec<i64>> {
        self.db
            .run(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT braindump_id FROM edge_provenance
                     WHERE edge_id = ?1 ORDER BY braindump_id",
                )?;
                let ids = stmt
                    .query_map(params![edge_id], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(ids)
            })
            .await
    }

    async fn edge_type_history(&self, edge_id: i64) -> Result<Vec<TypeHistoryEntry>> {
        self.db
            .run(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT seq_index, type_slug, created_at FROM edge_type_history
                     WHERE edge_id = ?1 ORDER BY seq_index",
                )?;
                let entries = stmt
                    .query_map(params![edge_id], |r| {
                        Ok(TypeHistoryEntry {
                            seq_index: r.get(0)?,
                            type_slug: r.get(1)?,
                            created_at: r.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(entries)
            })
            .await
    }

    async fn merge_suggestions(&self) -> Result<Vec<MergeSuggestion>> {
        self.db
            .run(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, kind, braindump_id, new_concept_label, new_concept_id,
                            existing_concept_id, similarity, status, created_at
                     FROM merge_suggestions ORDER BY id",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(MergeSuggestion {
                            id: r.get(0)?,
                            kind: r.get(1)?,
                            braindump_id: r.get(2)?,
                            new_concept_label: r.get(3)?,
                            new_concept_id: r.get(4)?,
                            existing_concept_id: r.get(5)?,
                            similarity: r.get::<_, f64>(6)? as f32,
                            status: r.get(7)?,
                            created_at: r.get(8)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn concept_id_for_label(&self, label: &str) -> Result<Option<i64>> {
        let label = label.to_string();
        self.db
            .run(move |conn| {
                let id = conn
                    .query_row(
                        "SELECT id FROM concepts WHERE label = ?1 ORDER BY id LIMIT 1",
                        params![label],
                        |r| r.get::<_, i64>(0),
                    )
                    .optional()?;
                Ok(id)
            })
            .await
    }

    async fn all_concepts(&self) -> Result<Vec<Concept>> {
        self.db
            .run(|conn| {
                let mut stmt =
                    conn.prepare("SELECT id, label, created_at FROM concepts ORDER BY id")?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(Concept {
                            id: r.get(0)?,
                            label: r.get(1)?,
                            created_at: r.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn all_edges_with_current_type(&self) -> Result<Vec<EdgeProjection>> {
        self.db
            .run(|conn| {
                let mut stmt = conn.prepare(&format!(
                    "SELECT e.id, e.source_concept_id, e.target_concept_id, e.original_type,
                            e.created_at, ({}) AS current_type
                     FROM edges e ORDER BY e.id",
                    current_type_subquery()
                ))?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(EdgeProjection {
                            id: r.get(0)?,
                            source_concept_id: r.get(1)?,
                            target_concept_id: r.get(2)?,
                            original_type: r.get(3)?,
                            created_at: r.get(4)?,
                            current_type: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn knn_concept(&self, query_vec: &[f32]) -> Result<Option<(i64, f32)>> {
        let query_vec = query_vec.to_vec();
        self.db
            .run(move |conn| knn_concept_conn(conn, &query_vec))
            .await
    }

    async fn knn_type(&self, query_vec: &[f32]) -> Result<Option<(String, f32)>> {
        let blob = vec_to_blob(query_vec);
        self.db
            .run(move |conn| {
                let row = conn
                    .prepare(
                        "SELECT ontology_id, distance FROM type_embeddings
                         WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
                    )?
                    .query_row(params![blob], |r| {
                        Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
                    })
                    .optional()?;
                match row {
                    Some((ontology_id, sim)) => {
                        let slug: String = conn.query_row(
                            "SELECT slug FROM ontology WHERE id = ?1",
                            params![ontology_id],
                            |r| r.get(0),
                        )?;
                        Ok(Some((slug, sim)))
                    }
                    None => Ok(None),
                }
            })
            .await
    }

    async fn knn_concepts(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        let blob = vec_to_blob(query_vec);
        self.db
            .run(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT concept_id, distance FROM concept_embeddings
                     WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![blob, limit as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
                })?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .await
    }

    async fn knn_braindumps(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        let blob = vec_to_blob(query_vec);
        self.db
            .run(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT braindump_id, distance FROM braindump_embeddings
                     WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
                )?;
                let rows = stmt.query_map(params![blob, limit as i64], |r| {
                    Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
                })?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .await
    }
}

/// In-memory adapter for tests: holds the graph state in HashMaps so every
/// read can be exercised without a SQLite connection. KNN is brute-force
/// cosine (the test paths use small N so the cost is fine). Gate on `test` and
/// the forward-looking `test-support` feature so integration-test crates (in
/// `backend/tests/`) can enable it.
///
/// Mutators (`add_concept`, `add_edge`, `set_concept_embedding`, …) are test
/// infrastructure: tests populate state directly without standing up the
/// accretion pipeline. #46's write-path trait methods will later be the real
/// mutators; these test mutators are acceptable per "InMemory adapter backs
/// reads with HashMaps".
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryGraphRepo {
    /// The set of braindump ids whose embedding is "stored" (retrieval
    /// backfill, ADR-0004). Maps to the embedding vector so KNN can run.
    braindump_embeddings: std::sync::Mutex<std::collections::HashMap<i64, Vec<f32>>>,
    /// `concept_id → Concept` (the full node set).
    concepts: std::sync::Mutex<std::collections::HashMap<i64, Concept>>,
    /// `edge_id → Edge`. Identity anchors on `(source, original_type, target)`
    /// (ADR-0002); `find_edge` scans by that key.
    edges: std::sync::Mutex<std::collections::HashMap<i64, Edge>>,
    /// `concept_id → Vec<braindump_id>` (ADR-0010 extraction provenance).
    concept_provenance: std::sync::Mutex<std::collections::HashMap<i64, Vec<i64>>>,
    /// `edge_id → Vec<braindump_id>` (ADR-0002 `asserted_by`).
    edge_provenance: std::sync::Mutex<std::collections::HashMap<i64, Vec<i64>>>,
    /// `edge_id → Vec<TypeHistoryEntry>` (ADR-0003 append-only log). Ordered by
    /// `seq_index`; the last entry is the current (projected) type.
    edge_type_history: std::sync::Mutex<std::collections::HashMap<i64, Vec<TypeHistoryEntry>>>,
    /// The pending + resolved merge suggestions (ADR-0001), ordered by id.
    merge_suggestions: std::sync::Mutex<Vec<MergeSuggestion>>,
    /// `(slug, label, description)` tuples ordered by `id` — the governed
    /// edge-type vocabulary.
    ontology: std::sync::Mutex<Vec<(String, String, String)>>,
    /// `concept_id → embedding` for KNN (ADR-0001 identity / ADR-0004 seed).
    concept_embeddings: std::sync::Mutex<std::collections::HashMap<i64, Vec<f32>>>,
    /// `(slug, embedding)` for type-embedding KNN (ADR-0003 dedup).
    type_embeddings: std::sync::Mutex<Vec<(String, Vec<f32>)>>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryGraphRepo {
    /// A fresh, empty in-memory graph. Every read returns empty/`None` until
    /// state is populated via the mutators.
    pub fn new() -> Self {
        Self {
            braindump_embeddings: std::sync::Mutex::new(std::collections::HashMap::new()),
            concepts: std::sync::Mutex::new(std::collections::HashMap::new()),
            edges: std::sync::Mutex::new(std::collections::HashMap::new()),
            concept_provenance: std::sync::Mutex::new(std::collections::HashMap::new()),
            edge_provenance: std::sync::Mutex::new(std::collections::HashMap::new()),
            edge_type_history: std::sync::Mutex::new(std::collections::HashMap::new()),
            merge_suggestions: std::sync::Mutex::new(Vec::new()),
            ontology: std::sync::Mutex::new(Vec::new()),
            concept_embeddings: std::sync::Mutex::new(std::collections::HashMap::new()),
            type_embeddings: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Mark a braindump's embedding as stored so a subsequent
    /// [`GraphRepo::braindump_embedding_stored`] returns `true`. Stores an
    /// empty vector — sufficient for the "is it stored?" check; for KNN use
    /// [`set_braindump_embedding`](Self::set_braindump_embedding).
    pub fn mark_braindump_embedding_stored(&self, braindump_id: i64) {
        self.braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(braindump_id, Vec::new());
    }

    /// Store a braindump's embedding vector (retrieval backfill + KNN,
    /// ADR-0004). Idempotent: a second call overwrites the first.
    pub fn set_braindump_embedding(&self, braindump_id: i64, vec: Vec<f32>) {
        self.braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(braindump_id, vec);
    }

    /// Add a concept to the in-memory graph (test mutator).
    pub fn add_concept(&self, concept: Concept) {
        self.concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(concept.id, concept);
    }

    /// Add an edge to the in-memory graph and seed its type history at
    /// `seq_index = 0` with `original_type` (ADR-0003: index 0 is the
    /// immutable original assertion). Idempotent: re-adding the same edge id
    /// overwrites the edge row but does NOT re-seed the type history.
    pub fn add_edge(&self, edge: Edge) {
        let mut edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
        let is_new = !edges.contains_key(&edge.id);
        edges.insert(edge.id, edge.clone());
        drop(edges);
        if is_new {
            self.edge_type_history
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .insert(
                    edge.id,
                    vec![TypeHistoryEntry {
                        seq_index: 0,
                        type_slug: edge.original_type.clone(),
                        created_at: edge.created_at,
                    }],
                );
        }
    }

    /// Append a new type-history entry to an edge (ADR-0003 refactor retag).
    /// `seq_index` is `max(existing) + 1`, so retags stack without
    /// overwriting.
    pub fn append_edge_type_history(&self, edge_id: i64, type_slug: &str, created_at: i64) {
        let mut history = self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let entries = history.entry(edge_id).or_default();
        let next_seq = entries.iter().map(|e| e.seq_index).max().unwrap_or(-1) + 1;
        entries.push(TypeHistoryEntry {
            seq_index: next_seq,
            type_slug: type_slug.to_string(),
            created_at,
        });
    }

    /// Record that `braindump_id` extracted `concept_id` (ADR-0010 extraction
    /// provenance). Idempotent: a duplicate insert is a no-op.
    pub fn add_concept_provenance(&self, concept_id: i64, braindump_id: i64) {
        let mut prov = self
            .concept_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let entries = prov.entry(concept_id).or_default();
        if !entries.contains(&braindump_id) {
            entries.push(braindump_id);
            entries.sort_unstable();
        }
    }

    /// Record that `braindump_id` asserts `edge_id` (ADR-0002 `asserted_by`).
    /// Idempotent: a duplicate insert is a no-op.
    pub fn add_edge_provenance(&self, edge_id: i64, braindump_id: i64) {
        let mut prov = self
            .edge_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let entries = prov.entry(edge_id).or_default();
        if !entries.contains(&braindump_id) {
            entries.push(braindump_id);
            entries.sort_unstable();
        }
    }

    /// Add a merge suggestion (ADR-0001) to the in-memory queue. Ordered by
    /// `id` on read.
    pub fn add_merge_suggestion(&self, suggestion: MergeSuggestion) {
        self.merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push(suggestion);
    }

    /// Add a governed edge type to the in-memory ontology (ordered by `id` =
    /// insertion order).
    pub fn add_ontology_type(&self, slug: &str, label: &str, description: &str) {
        self.ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push((slug.to_string(), label.to_string(), description.to_string()));
    }

    /// Store a concept's embedding vector (identity + retrieval seed,
    /// ADR-0001 / ADR-0004). Idempotent: a second call overwrites the first.
    pub fn set_concept_embedding(&self, concept_id: i64, vec: Vec<f32>) {
        self.concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(concept_id, vec);
    }

    /// Store a type embedding (ADR-0003 dedup). Idempotent: a second call for
    /// the same slug overwrites the first.
    pub fn set_type_embedding(&self, slug: &str, vec: Vec<f32>) {
        let mut embeddings = self
            .type_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        if let Some(entry) = embeddings.iter_mut().find(|(s, _)| s == slug) {
            entry.1 = vec;
        } else {
            embeddings.push((slug.to_string(), vec));
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Default for InMemoryGraphRepo {
    fn default() -> Self {
        Self::new()
    }
}

/// Brute-force cosine similarity. The Sqlite vec0 table returns cosine
/// DISTANCE (`1 − similarity`); the trait methods return similarity
/// (`1 − distance`), so the in-memory adapter returns `cosine_similarity`
/// directly. Zero-vector or mismatched-dim inputs return `0.0` (no NaN).
#[cfg(any(test, feature = "test-support"))]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

#[cfg(any(test, feature = "test-support"))]
#[async_trait]
impl GraphRepo for InMemoryGraphRepo {
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool> {
        let stored = self
            .braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        Ok(stored.contains_key(&braindump_id))
    }

    async fn ontology_types(&self) -> Result<Vec<(String, String, String)>> {
        Ok(self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone())
    }

    async fn get_concept(&self, id: i64) -> Result<Option<Concept>> {
        Ok(self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&id)
            .cloned())
    }

    async fn concept_provenance(&self, concept_id: i64) -> Result<Vec<i64>> {
        Ok(self
            .concept_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&concept_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn find_edge(
        &self,
        source_id: i64,
        original_type: &str,
        target_id: i64,
    ) -> Result<Option<Edge>> {
        Ok(self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .find(|e| {
                e.source_concept_id == source_id
                    && e.original_type == original_type
                    && e.target_concept_id == target_id
            })
            .cloned())
    }

    async fn edge_provenance(&self, edge_id: i64) -> Result<Vec<i64>> {
        Ok(self
            .edge_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&edge_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn edge_type_history(&self, edge_id: i64) -> Result<Vec<TypeHistoryEntry>> {
        Ok(self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&edge_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn merge_suggestions(&self) -> Result<Vec<MergeSuggestion>> {
        let mut suggestions = self
            .merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        suggestions.sort_by_key(|s| s.id);
        Ok(suggestions)
    }

    async fn concept_id_for_label(&self, label: &str) -> Result<Option<i64>> {
        Ok(self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .filter(|c| c.label == label)
            .map(|c| c.id)
            .min())
    }

    async fn all_concepts(&self) -> Result<Vec<Concept>> {
        let mut concepts: Vec<Concept> = self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .cloned()
            .collect();
        concepts.sort_by_key(|c| c.id);
        Ok(concepts)
    }

    async fn all_edges_with_current_type(&self) -> Result<Vec<EdgeProjection>> {
        let edges = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let history = self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let mut projections: Vec<EdgeProjection> = edges
            .into_values()
            .map(|e| {
                let current_type = history
                    .get(&e.id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .max_by_key(|h| h.seq_index)
                            .map(|h| h.type_slug.clone())
                    })
                    .unwrap_or_default();
                EdgeProjection {
                    id: e.id,
                    source_concept_id: e.source_concept_id,
                    target_concept_id: e.target_concept_id,
                    original_type: e.original_type,
                    current_type,
                    created_at: e.created_at,
                }
            })
            .collect();
        projections.sort_by_key(|e| e.id);
        Ok(projections)
    }

    async fn knn_concept(&self, query_vec: &[f32]) -> Result<Option<(i64, f32)>> {
        let embeddings = self
            .concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        Ok(embeddings
            .into_iter()
            .map(|(id, vec)| (id, cosine_similarity(query_vec, &vec)))
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .filter(|(_, sim)| *sim > 0.0))
    }

    async fn knn_type(&self, query_vec: &[f32]) -> Result<Option<(String, f32)>> {
        let embeddings = self
            .type_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        Ok(embeddings
            .into_iter()
            .map(|(slug, vec)| (slug, cosine_similarity(query_vec, &vec)))
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .filter(|(_, sim)| *sim > 0.0))
    }

    async fn knn_concepts(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        let mut hits: Vec<(i64, f32)> = self
            .concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .map(|(id, vec)| (*id, cosine_similarity(query_vec, vec)))
            .filter(|(_, sim)| *sim > 0.0)
            .collect();
        hits.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        Ok(hits)
    }

    async fn knn_braindumps(&self, query_vec: &[f32], limit: usize) -> Result<Vec<(i64, f32)>> {
        let mut hits: Vec<(i64, f32)> = self
            .braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .map(|(id, vec)| (*id, cosine_similarity(query_vec, vec)))
            .filter(|(_, sim)| *sim > 0.0)
            .collect();
        hits.sort_by(|(_, a), (_, b)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(limit);
        Ok(hits)
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

    /// Read paths previously requiring an in-memory SQLite harness now run
    /// against `InMemoryGraphRepo` alone: populate concepts + edges + type
    /// history via the test mutators, then assert the snapshot read returns
    /// exactly what was inserted, with `current_type` projected from the
    /// append-only type history (ADR-0003).
    #[tokio::test]
    async fn in_memory_all_concepts_and_edges_with_current_type_after_population() {
        let repo = InMemoryGraphRepo::new();
        // Seed two concepts + one edge (the canonical Maria —[endangers]→ Q3
        // pair). The edge wears its original type as current.
        repo.add_concept(Concept {
            id: 1,
            label: "Maria".into(),
            created_at: 100,
        });
        repo.add_concept(Concept {
            id: 2,
            label: "Q3 launch".into(),
            created_at: 100,
        });
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "endangers".into(),
            created_at: 100,
        });

        let concepts = repo.all_concepts().await.unwrap();
        assert_eq!(concepts.len(), 2, "both concepts returned");
        assert_eq!(concepts[0].id, 1);
        assert_eq!(concepts[1].id, 2, "concepts ordered by id");

        let edges = repo.all_edges_with_current_type().await.unwrap();
        assert_eq!(edges.len(), 1, "the one edge returned");
        let e = &edges[0];
        assert_eq!(e.id, 10);
        assert_eq!(e.source_concept_id, 1);
        assert_eq!(e.target_concept_id, 2);
        assert_eq!(e.original_type, "endangers");
        assert_eq!(
            e.current_type, e.original_type,
            "fresh edge: current type == original (projected from history index 0)"
        );

        // ADR-0003: a refactor retag appends to the type history; the
        // projected current type flips, original stays immutable.
        repo.append_edge_type_history(10, "affects", 200);
        let edges_after = repo.all_edges_with_current_type().await.unwrap();
        assert_eq!(
            edges_after[0].current_type, "affects",
            "current type projected from the last history entry"
        );
        assert_eq!(
            edges_after[0].original_type, "endangers",
            "original assertion immutable across retag"
        );

        // `find_edge` looks up by (source, original_type, target) and returns
        // the immutable Edge (no current_type — that's a projection).
        let found = repo
            .find_edge(1, "endangers", 2)
            .await
            .unwrap()
            .expect("edge found by identity key");
        assert_eq!(found.id, 10);
        assert_eq!(found.original_type, "endangers");
        let none = repo.find_edge(1, "helps", 2).await.unwrap();
        assert!(none.is_none(), "no edge matches a different type");

        // Type history read mirrors what the projection consumes.
        let history = repo.edge_type_history(10).await.unwrap();
        assert_eq!(history.len(), 2, "original + retag");
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
        assert_eq!(history[1].seq_index, 1);
        assert_eq!(history[1].type_slug, "affects");
    }

    /// KNN over the in-memory adapter is brute-force cosine: the closest
    /// concept embedding wins, with similarity matching the Sqlite vec0
    /// convention (1 − distance).
    #[tokio::test]
    async fn in_memory_knn_concept_returns_nearest_by_cosine() {
        let repo = InMemoryGraphRepo::new();
        // Empty graph → no hit.
        assert!(repo.knn_concept(&[1.0, 0.0]).await.unwrap().is_none());

        repo.add_concept(Concept {
            id: 1,
            label: "A".into(),
            created_at: 0,
        });
        repo.add_concept(Concept {
            id: 2,
            label: "B".into(),
            created_at: 0,
        });
        repo.set_concept_embedding(1, vec![1.0, 0.0]);
        repo.set_concept_embedding(2, vec![0.0, 1.0]);

        // Query identical to concept 1 → similarity 1.0, returns concept 1.
        let (id, sim) = repo.knn_concept(&[1.0, 0.0]).await.unwrap().expect("a hit");
        assert_eq!(id, 1);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "identical vector → cosine 1.0, got {sim}"
        );

        // Top-K concepts: query closer to concept 1 → concept 1 ranks first.
        let top = repo.knn_concepts(&[0.9, 0.1], 5).await.unwrap();
        assert_eq!(top.len(), 2, "both concepts returned");
        assert_eq!(top[0].0, 1, "concept 1 ranks first (closer)");
        assert!(top[0].1 > top[1].1, "sorted by similarity descending");
    }

    /// `ontology_slugs` is derived from `ontology_types` so the slug-only read
    /// and the full-row read cannot drift apart. Verified against the in-memory
    /// adapter (the Sqlite adapter has the same default-method derivation).
    #[tokio::test]
    async fn in_memory_ontology_slugs_derived_from_ontology_types() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
        repo.add_ontology_type("helps", "Helps", "A benefits B.");
        let slugs = repo.ontology_slugs().await.unwrap();
        assert_eq!(slugs, vec!["causes".to_string(), "helps".to_string()]);
        let types = repo.ontology_types().await.unwrap();
        assert_eq!(types.len(), 2);
        assert_eq!(
            types[0],
            ("causes".into(), "Causes".into(), "A brings about B.".into())
        );
    }
}
