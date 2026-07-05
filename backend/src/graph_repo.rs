//! The graph-repository seam (issues #44 / #45 / #46).
//!
//! Every read AND write against the knowledge graph goes through [`GraphRepo`]
//! so call sites depend on the interface, not the storage adapter. Production
//! wires [`SqliteGraphRepo`] (delegating to [`Db::with_conn`](crate::db::Db)); tests
//! wire [`InMemoryGraphRepo`] so every read and write can be exercised without
//! a SQLite connection — the graph becomes hermetic.
//!
//! After #45 the SQL that defines "the current type is the projected state of
//! the type history" (ADR-0003) and the byte layout sqlite-vec expects both
//! live here — in the Sqlite adapter — instead of being copy-pasted across
//! `graph`, `ontology`, `retrieval`, and `snapshot`. After #46 the write paths
//! — Braindump ingest, Concept/Edge accretion, the deletion cascade, and the
//! merge queue — also live here: the Sqlite adapter owns every
//! INSERT/UPDATE/DELETE and the BEGIN/COMMIT/ROLLBACK shape (one [`run_txn`]
//! helper), and the in-memory adapter mutates HashMaps. The free-function read
//! and write helpers in `graph.rs` / `ontology.rs` / `braindump.rs` remain as
//! one-line delegators to this trait so existing callers (including the
//! integration tests under `backend/tests/`) keep compiling; #48 removes them
//! once every caller is migrated to take a `&dyn GraphRepo` directly.
//!
//! [`run_txn`]: SqliteGraphRepo::run_txn

use std::collections::{HashMap, HashSet, VecDeque};

use async_trait::async_trait;
use petgraph::stable_graph::{NodeIndex, StableUnGraph};
use rusqlite::{params, OptionalExtension};

use crate::braindump::Braindump;
use crate::chat_inference::{
    ChatInferenceProposal, EvidenceEdge, InferenceAssertion, ThematicSnapshot, STATUS_ENDORSED,
    STATUS_PENDING, STATUS_REJECTED, STRUCTURAL_MODE, THEMATIC_MODE,
};
use crate::db::{now_seconds, Db};
use crate::delta::{DeltaEdge, GraphDelta, RetaggedEdge};
use crate::error::{Error, Result};
use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
use crate::graph::{
    Concept, Edge, EdgeProjection, IngestOutcome, MergeSuggestion, TypeHistoryEntry,
    ACCRETION_SIMILARITY, SUGGESTION_FLOOR_SIMILARITY,
};
use crate::llm::Llm;
use crate::ontology::{RefactorOutcome, TypeProposal};
use crate::retrieval::{
    BraindumpSource, RetrievalMode, RetrievalResult, RetrievedBraindump, RetrievedEdge,
    BRAINDUMP_TOP_K, EXPAND_DEPTH, SEED_SIMILARITY_FLOOR, SEED_TOP_K,
};

/// The canonical current-type projection SQL fragment (ADR-0003): the last
/// `edge_type_history` entry, correlated on the outer edges alias `e`.
///
/// Lives in the Sqlite adapter's home so the projection lives in one place.
/// Private after #48 — no domain module calls it directly anymore.
fn current_type_subquery() -> &'static str {
    "SELECT type_slug FROM edge_type_history WHERE edge_id = e.id ORDER BY seq_index DESC LIMIT 1"
}

/// f32 slice → little-endian byte blob, the on-disk format sqlite-vec expects.
///
/// Lives in the Sqlite adapter's home so the byte layout is defined once.
/// Private after #48 — no domain module calls it directly anymore.
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
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
/// Lives in the Sqlite adapter as a private helper so the accretion write-path
/// trait impl can call it synchronously inside the transaction — the KNN must
/// see the post-retraction state (embeddings for vanished concepts are deleted
/// before identity resolution runs), so it cannot be lifted out of the closure
/// to call the async trait method.
fn knn_concept_conn(conn: &rusqlite::Connection, query_vec: &[f32]) -> Result<Option<(i64, f32)>> {
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

// --- write-path `*_conn` helpers (moved from `graph.rs` in issue #46) ---
//
// These synchronous helpers run inside the Sqlite adapter's `run_txn`
// closures. Private after #48 — no domain module calls them.

/// Insert an edge row and return its surrogate id (ADR-0002).
fn insert_edge_conn(
    conn: &rusqlite::Connection,
    source_id: i64,
    target_id: i64,
    original_type: &str,
) -> Result<i64> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO edges (source_concept_id, target_concept_id, original_type, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![source_id, target_id, original_type, created_at],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Initialize the append-only type history at index 0 = the LLM's original
/// assertion (ADR-0003). The edge's current type is a projection off this log.
fn init_type_history_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    type_slug: &str,
) -> Result<()> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
         VALUES (?1, 0, ?2, ?3)",
        params![edge_id, type_slug, created_at],
    )?;
    Ok(())
}

/// Look up an edge id by its identity key `(source, original_type, target)`
/// (ADR-0002). `None` if no edge matches.
fn find_edge_id_conn(
    conn: &rusqlite::Connection,
    source_id: i64,
    original_type: &str,
    target_id: i64,
) -> Result<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT id FROM edges
             WHERE source_concept_id = ?1 AND original_type = ?2 AND target_concept_id = ?3",
            params![source_id, original_type, target_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(id)
}

/// Whether `source —[type]→ target` exists wearing `type` as its current
/// projected type (ADR-0003) — the structural-inference traversability check.
fn edge_exists_with_current_type_conn(
    conn: &rusqlite::Connection,
    source_id: i64,
    type_slug: &str,
    target_id: i64,
) -> Result<bool> {
    let exists = conn
        .query_row(
            &format!(
                "SELECT 1 FROM edges e WHERE e.source_concept_id = ?1
                 AND e.target_concept_id = ?2 AND ({}) = ?3",
                current_type_subquery()
            ),
            params![source_id, target_id, type_slug],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

/// Whether a concept with `id` exists in the graph.
fn concept_exists_conn(conn: &rusqlite::Connection, id: i64) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM concepts WHERE id = ?1",
        params![id],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// The governed edge-type slugs the LLM draws from (connection-scoped).
fn ontology_slugs_conn(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT slug FROM ontology ORDER BY id")?;
    let slugs = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(slugs)
}

/// Fold `fold_id` into `keep_id` (the survivor) inside an open transaction:
/// repoint/merge edges touching the fold concept, union extraction provenance,
/// drop the fold concept's embedding (vec0 has no FK cascade), then delete the
/// fold concept — its remaining provenance and any merge suggestions
/// referencing it cascade away (ADR-0001 / ADR-0010).
fn merge_concepts_conn(conn: &rusqlite::Connection, keep_id: i64, fold_id: i64) -> Result<()> {
    // Edges touching the fold concept: merge duplicates (union provenance) and
    // repoint the rest onto the survivor. Iterated in Rust because the edges
    // table's UNIQUE (source, original_type, target) would otherwise trip on a
    // straight UPDATE when a duplicate already exists on the survivor.
    let fold_edges: Vec<(i64, i64, i64, String)> = conn
        .prepare(
            "SELECT id, source_concept_id, target_concept_id, original_type
             FROM edges WHERE source_concept_id = ?1 OR target_concept_id = ?1",
        )?
        .query_map(params![fold_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (edge_id, src, tgt, otype) in fold_edges {
        let new_src = if src == fold_id { keep_id } else { src };
        let new_tgt = if tgt == fold_id { keep_id } else { tgt };
        let collision = conn
            .query_row(
                "SELECT id FROM edges
                 WHERE source_concept_id = ?1 AND original_type = ?2
                   AND target_concept_id = ?3 AND id != ?4",
                params![new_src, &otype, new_tgt, edge_id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?;
        if let Some(keeper_edge_id) = collision {
            // Same (source, original_type, target) already on the survivor →
            // union provenance onto the keeper, then drop the fold edge (its
            // type-history + remaining provenance cascade).
            conn.execute(
                "INSERT OR IGNORE INTO edge_provenance (edge_id, braindump_id)
                 SELECT ?1, braindump_id FROM edge_provenance WHERE edge_id = ?2",
                params![keeper_edge_id, edge_id],
            )?;
            conn.execute("DELETE FROM edges WHERE id = ?1", params![edge_id])?;
        } else {
            conn.execute(
                "UPDATE edges
                 SET source_concept_id = ?1, target_concept_id = ?2
                 WHERE id = ?3",
                params![new_src, new_tgt, edge_id],
            )?;
        }
    }

    // Union extraction provenance: the fold concept's extractors accrete onto
    // the survivor (ADR-0010: a merged concept's provenance is the union).
    conn.execute(
        "INSERT OR IGNORE INTO concept_provenance (concept_id, braindump_id)
         SELECT ?1, braindump_id FROM concept_provenance WHERE concept_id = ?2",
        params![keep_id, fold_id],
    )?;
    // The vec0 concept_embeddings table has no FK cascade — clean manually.
    conn.execute(
        "DELETE FROM concept_embeddings WHERE concept_id = ?1",
        params![fold_id],
    )?;
    // Delete the fold concept; cascades drop its remaining provenance, any
    // edges still referencing it (none — all repointed above), and merge
    // suggestions that reference it as new/existing (the approved one included).
    conn.execute("DELETE FROM concepts WHERE id = ?1", params![fold_id])?;
    Ok(())
}

/// Transition a row's `status` + `resolved_at` inside an open transaction —
/// the canonical HITL resolution pattern shared by ontology governance
/// (ADR-0003 `type_proposals`) and chat-inference endorse/reject (ADR-0006
/// `chat_inference_proposals`). Maps 0 rows updated to `NotFound` (row
/// missing) or `Conflict` (row exists but is not `pending`). When
/// `pending_guard` is `true` the UPDATE includes `AND status = 'pending'` so
/// a non-pending row yields 0 rows; when `false` the UPDATE is unguarded (for
/// sites that pre-check status before the transaction — approve / endorse).
/// Applied in #47 at the ontology approve/reject and chat-inference
/// endorse/reject sites; the merge-queue sites use DELETE semantics (not
/// UPDATE) and so do not route through this helper.
fn transition_status_conn(
    conn: &rusqlite::Connection,
    table: &str,
    id: i64,
    new_status: &str,
    pending_guard: bool,
) -> Result<()> {
    let now = now_seconds();
    let sql = if pending_guard {
        format!(
            "UPDATE {table} SET status = ?1, resolved_at = ?2 \
             WHERE id = ?3 AND status = 'pending'"
        )
    } else {
        format!("UPDATE {table} SET status = ?1, resolved_at = ?2 WHERE id = ?3")
    };
    let n = conn.execute(&sql, params![new_status, now, id])?;
    if n == 0 {
        let exists_sql = format!("SELECT status FROM {table} WHERE id = ?1");
        match conn
            .query_row(&exists_sql, params![id], |r| r.get::<_, String>(0))
            .optional()?
        {
            None => Err(Error::NotFound(format!("{table} {id} not found"))),
            Some(status) => Err(Error::Conflict(format!(
                "{table} {id} is `{status}`, not `pending`"
            ))),
        }
    } else {
        Ok(())
    }
}

// --- chat-inference `*_conn` helpers (moved from `chat_inference.rs` in #47) ---
//
// Private to the adapter — no domain module calls them after #47; the trait
// methods own the propose/endorse/reject flows.

/// Insert a chat-inference provenance row (ADR-0006 origin-typed). The `mode`
/// origin-tags the assertion; `snapshot_id` is the frozen Thematic Snapshot for
/// thematic assertions (ADR-0009), `None` for structural.
fn insert_inference_provenance_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    chat_inference_id: i64,
    mode: &str,
    snapshot_id: Option<i64>,
) -> Result<()> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT OR IGNORE INTO edge_inference_provenance
            (edge_id, chat_inference_id, mode, snapshot_id, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![edge_id, chat_inference_id, mode, snapshot_id, created_at],
    )?;
    Ok(())
}

/// Compute the braindump ids whose edges formed the thematic density of a
/// cluster (ADR-0009): the distinct braindumps that asserted edges where BOTH
/// endpoints are in the cluster and the edge is not a self-edge.
fn compute_cluster_braindump_ids_conn(
    conn: &rusqlite::Connection,
    cluster_concept_ids: &[i64],
) -> Result<Vec<i64>> {
    if cluster_concept_ids.is_empty() {
        return Ok(Vec::new());
    }
    let cluster_json = serde_json::to_string(cluster_concept_ids)
        .map_err(|e| Error::internal(format!("encode cluster_concept_ids: {e}")))?;
    let mut stmt = conn.prepare(
        "SELECT DISTINCT ep.braindump_id
         FROM edge_provenance ep
         JOIN edges e ON ep.edge_id = e.id
         WHERE e.source_concept_id != e.target_concept_id
           AND e.source_concept_id IN (SELECT value FROM json_each(?1))
           AND e.target_concept_id IN (SELECT value FROM json_each(?1))
         ORDER BY ep.braindump_id",
    )?;
    let ids = stmt
        .query_map(params![cluster_json], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

// --- ontology `*_conn` helpers (moved from `ontology.rs` in #47) ---

/// Whether a slug already exists in the ontology (connection-scoped).
fn ontology_slug_exists_conn(conn: &rusqlite::Connection, slug: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ontology WHERE slug = ?1",
        params![slug],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Append a new type-history entry to an edge (ADR-0003). `seq_index` is
/// `max(existing) + 1`, so refactors stack without overwriting.
fn append_type_history_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    type_slug: &str,
) -> Result<()> {
    let created_at = now_seconds();
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq_index), -1) + 1 FROM edge_type_history WHERE edge_id = ?1",
            params![edge_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![edge_id, next_seq, type_slug, created_at],
    )?;
    Ok(())
}

// --- retrieval helpers (moved from `retrieval.rs` in #47) ---
//
// The petgraph BFS is private to the retrieval implementation; no other module
// imports petgraph. `bfs_expand` takes pre-loaded concept labels + edges so
// both adapters can call it after loading from their respective stores.

struct RetrievalEdgeInfo {
    edge_type: String,
    source_concept_id: i64,
    target_concept_id: i64,
}

/// Build the typed-edge graph and BFS from the seed concepts up to
/// [`EXPAND_DEPTH`] hops (undirected: incoming + outgoing). Returns each visited
/// concept's minimum hop distance from a seed, and the edges in the traversed
/// subgraph. petgraph is private to this function — no other module imports it.
fn bfs_expand(
    concept_labels: &HashMap<i64, String>,
    edges: &[RetrievalEdgeInfo],
    seeds: &[(i64, f32)],
) -> (HashMap<i64, usize>, Vec<RetrievedEdge>) {
    let mut graph: StableUnGraph<i64, RetrievalEdgeInfo> = StableUnGraph::default();
    let mut node_index: HashMap<i64, NodeIndex> = HashMap::new();
    for &cid in concept_labels.keys() {
        let idx = graph.add_node(cid);
        node_index.insert(cid, idx);
    }
    for info in edges {
        if let (Some(&s), Some(&t)) = (
            node_index.get(&info.source_concept_id),
            node_index.get(&info.target_concept_id),
        ) {
            graph.add_edge(
                s,
                t,
                RetrievalEdgeInfo {
                    edge_type: info.edge_type.clone(),
                    source_concept_id: info.source_concept_id,
                    target_concept_id: info.target_concept_id,
                },
            );
        }
    }

    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut concept_hops: HashMap<i64, usize> = HashMap::new();
    let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();
    for &(seed_cid, _) in seeds {
        if let Some(&idx) = node_index.get(&seed_cid) {
            if visited.insert(idx) {
                concept_hops.insert(seed_cid, 0);
                queue.push_back((idx, 0));
            }
        }
    }
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= EXPAND_DEPTH {
            continue;
        }
        for neighbor in graph.neighbors(node) {
            if visited.insert(neighbor) {
                if let Some(&cid) = graph.node_weight(neighbor) {
                    concept_hops.entry(cid).or_insert(depth + 1);
                }
                queue.push_back((neighbor, depth + 1));
            }
        }
    }

    let traversed: Vec<RetrievedEdge> = graph
        .edge_indices()
        .filter_map(|e| {
            let (s, t) = graph.edge_endpoints(e)?;
            if visited.contains(&s) && visited.contains(&t) {
                let info = graph.edge_weight(e)?;
                Some(RetrievedEdge {
                    source_concept_id: info.source_concept_id,
                    source_concept_label: concept_labels
                        .get(&info.source_concept_id)
                        .cloned()
                        .unwrap_or_default(),
                    target_concept_id: info.target_concept_id,
                    target_concept_label: concept_labels
                        .get(&info.target_concept_id)
                        .cloned()
                        .unwrap_or_default(),
                    edge_type: info.edge_type.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    (concept_hops, traversed)
}

/// Load a braindump row by id from a SQLite connection. `None` if no row
/// matches. Used by the retrieval subgraph-collection, backfill, and
/// no-seed-fallback paths.
fn load_braindump_row_conn(
    conn: &rusqlite::Connection,
    id: i64,
) -> Result<Option<(i64, String, String, i64)>> {
    let row = conn
        .query_row(
            "SELECT id, verbatim, cleaned, created_at FROM braindumps WHERE id = ?1",
            params![id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
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

    // --- write paths (issue #46) ---

    /// Persist a new braindump with its verbatim and cleaned rendering
    /// (ADR-0007). The granular write behind the ingest orchestration; returns
    /// the stored row with the surrogate id and `created_at` filled in. No
    /// transaction — a single INSERT.
    async fn insert_braindump(&self, verbatim: &str, cleaned: &str) -> Result<Braindump>;

    /// Look up a braindump by id. `None` if no row matches.
    async fn get_braindump(&self, id: i64) -> Result<Option<Braindump>>;

    /// Overwrite the verbatim in place (error-correction, ADR-0007) and store
    /// the re-cleaned rendering. The id and `created_at` are untouched. Returns
    /// the updated row, or `None` if no braindump with `id` exists.
    async fn update_braindump(
        &self,
        id: i64,
        verbatim: String,
        cleaned: String,
    ) -> Result<Option<Braindump>>;

    /// The Concept/Edge accretion pipeline (ADR-0001 / ADR-0002 / ADR-0003 /
    /// ADR-0007 / ADR-0010): retract this braindump's prior extraction, store
    /// its embedding, resolve each concept by embedding KNN (accrete /
    /// suggest / create), accrete edges by `(source, original_type, target)`,
    /// init type history at index 0, and write provenance — all inside one
    /// transaction. The embedding *computation* (LLM network call) runs in
    /// the caller; the precomputed vectors are passed in so the trait method
    /// owns only the synchronous storage work that must commit atomically.
    async fn ingest_extraction(
        &self,
        braindump_id: i64,
        braindump_vec: Vec<f32>,
        extraction: ExtractionResult,
        concept_vecs: Vec<Vec<f32>>,
    ) -> Result<IngestOutcome>;

    /// Delete a braindump and cascade through the graph (ADR-0002 / ADR-0007 /
    /// ADR-0010). Drops the braindump's id from every concept's extraction
    /// provenance and every edge's `asserted_by`; a concept vanishes when its
    /// last extracting braindump is removed, an edge vanishes when its last
    /// asserter is removed. Returns `false` if no braindump with `id` exists.
    async fn delete_braindump(&self, braindump_id: i64) -> Result<bool>;

    /// Approve a pending concept merge suggestion (ADR-0001 / ADR-0010): fold
    /// the `new_concept_id` into the `existing_concept_id` — union extraction
    /// provenance, repoint edges from the fold concept onto the survivor, and
    /// drop the fold concept and the suggestion. `NotFound` if the suggestion
    /// does not exist.
    async fn approve_merge_suggestion(&self, suggestion_id: i64) -> Result<()>;

    /// Reject a pending concept merge suggestion (ADR-0001): keep the two
    /// concepts separate and drop the suggestion. `NotFound` if the suggestion
    /// does not exist.
    async fn reject_merge_suggestion(&self, suggestion_id: i64) -> Result<()>;

    // --- chat write-back (issue #47, ADR-0006) ---
    //
    // All pure-DB: the LLM that produces the proposal info (path, cluster,
    // rationale) lives in the chat route, not in chat_inference.rs. The trait
    // methods own the propose→HITL→endorse/reject storage flow. The
    // `InferenceProposer` trait (architecture-review candidate 3) becomes the
    // natural next step after this slice — splitting Structural and Thematic
    // proposal *generation* (the LLM side) behind a separate trait.

    /// Propose a structural inference (ADR-0006): store a pending proposal for
    /// a direct edge summarizing a traversable multi-hop path. Validates the
    /// proposed type is a governed ontology slug and every evidence-path hop is
    /// a real edge wearing the stated type as its current projected type.
    async fn propose_structural_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        evidence_path: Vec<EvidenceEdge>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal>;

    /// Propose a thematic inference (ADR-0006 + ADR-0009): store a pending
    /// proposal for a new edge bridging cluster-mates, with a frozen Thematic
    /// Snapshot. Validates the proposed type, cluster concepts, and computed
    /// braindump evidence.
    async fn propose_thematic_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        cluster_concept_ids: Vec<i64>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal>;

    /// Endorse a pending proposal (ADR-0006): persist the edge + inference
    /// provenance + type history, flip status to endorsed. `NotFound` if the
    /// proposal does not exist; `Conflict` if not pending.
    async fn endorse_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal>;

    /// Reject a pending proposal (ADR-0006): keep the graph untouched, mark
    /// the proposal rejected. `NotFound` if missing; `Conflict` if not pending.
    async fn reject_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal>;

    /// List all chat-inference proposals, oldest first.
    async fn list_inference_proposals(&self) -> Result<Vec<ChatInferenceProposal>>;

    /// Look up a single chat-inference proposal by id. `None` if no row matches.
    async fn get_inference_proposal(&self, id: i64) -> Result<Option<ChatInferenceProposal>>;

    /// The chat-inference assertions backing an edge (ADR-0006 origin-typed
    /// provenance).
    async fn edge_inference_asserted_by(&self, edge_id: i64) -> Result<Vec<InferenceAssertion>>;

    // --- ontology governance + refactor (issue #47, ADR-0003) ---
    //
    // Governance (propose/approve/reject) is pure-DB: the LLM embedding
    // computation runs in the wrapper. The refactor takes `&dyn Llm` (allowed
    // deviation per #47's design rule) because it interleaves LLM-per-edge
    // re-classification with DB reads; the InMemoryGraphRepo impl works with
    // FakeLlm so tests are hermetic.

    /// List all type proposals, oldest first.
    async fn list_type_proposals(&self) -> Result<Vec<TypeProposal>>;

    /// Look up a single type proposal by id. `None` if no row matches.
    async fn get_type_proposal(&self, id: i64) -> Result<Option<TypeProposal>>;

    /// Insert a type proposal row (pure-DB). The wrapper computes the embedding
    /// + KNN near-match + status; this method owns only the INSERT.
    #[allow(clippy::too_many_arguments)]
    async fn insert_type_proposal(
        &self,
        slug: String,
        label: String,
        description: String,
        merge_of: Option<String>,
        status: String,
        near_match_slug: Option<String>,
        near_match_similarity: Option<f32>,
    ) -> Result<TypeProposal>;

    /// Approve a pending type proposal (ADR-0003): add the type to the ontology,
    /// store its type-embedding, mark the proposal approved. The wrapper computes
    /// the embedding; this method owns the atomic INSERT+UPDATE inside one
    /// transaction.
    async fn approve_type_proposal(
        &self,
        id: i64,
        slug: String,
        label: String,
        description: String,
        type_vec: Vec<f32>,
    ) -> Result<()>;

    /// Reject a pending type proposal (ADR-0003): mark rejected, no ontology
    /// change. `NotFound` if missing; `Conflict` if not pending.
    async fn reject_type_proposal(&self, id: i64) -> Result<()>;

    /// The projected current type of an edge: the last entry of its append-only
    /// type history (ADR-0003). `None` if no history.
    async fn current_edge_type(&self, edge_id: i64) -> Result<Option<String>>;

    /// Every edge id whose projected current type is `slug`. The refactor
    /// targets these edges.
    async fn edges_with_current_type(&self, slug: &str) -> Result<Vec<i64>>;

    /// The (source label, target label, current type) for an edge — the prompt
    /// payload for the refactor LLM.
    async fn edge_endpoints_and_type(&self, edge_id: i64) -> Result<(String, String, String)>;

    /// Run the ontology refactor (ADR-0003): re-classify every edge of the
    /// merged type against the new vocabulary, appending the LLM's chosen slug
    /// to each edge's type history. Takes `&dyn Llm` (allowed deviation: the
    /// LLM-per-edge interleaving can't be split without duplicating logic); the
    /// InMemoryGraphRepo impl works with FakeLlm.
    async fn run_refactor(&self, llm: &dyn Llm, proposal: &TypeProposal)
        -> Result<RefactorOutcome>;

    // --- retrieval pipeline (issue #47, ADR-0004) ---
    //
    // Pure-DB: the wrapper computes the query embedding (LLM); the trait method
    // takes the precomputed `query_vec` and owns the full seed→expand→collect→
    // backfill flow. The petgraph BFS is private to the adapter's impl.

    /// Run seed-then-expand retrieval (or the no-seed fallback) for a
    /// precomputed query vector. Returns ranked braindumps plus the traversed
    /// edge paths.
    async fn retrieve(&self, query_vec: &[f32]) -> Result<RetrievalResult>;

    // --- issue #48: additional reads/writes migrated from domain modules ---

    /// Every edge's `(source_concept_id, target_concept_id)` endpoints, ordered
    /// by edge id — the topology input for Louvain clustering (ADR-0008). No
    /// type information: the partition is type-agnostic (each typed edge
    /// contributes unit weight regardless of its type).
    async fn all_edge_endpoints(&self) -> Result<Vec<(i64, i64)>>;

    /// Compute the graph delta since `since` (issue #28): additions, deletions
    /// (tombstones), and retags. The cursor is `now_seconds()` at query time.
    async fn graph_delta(&self, since: i64) -> Result<GraphDelta>;

    /// Ontology types missing a type-embedding (ADR-0003 dedup). Returns
    /// `(ontology_id, slug, label, description)` for each type that has no row
    /// in the type-embedding collection. Used by the startup seed.
    async fn missing_type_rows(&self) -> Result<Vec<(i64, String, String, String)>>;

    /// Store a type-embedding for `ontology_id` (ADR-0003 dedup). Idempotent:
    /// `INSERT OR IGNORE`.
    async fn store_type_embedding(&self, ontology_id: i64, vec: Vec<f32>) -> Result<()>;
}

/// Production adapter: delegates to [`Db::with_conn`](crate::db::Db::with_conn)
/// against the in-process `sqlite-vec`, so the single-connection transaction
/// guarantees of `Db` (ADR-0001) are preserved. Owns the SQL bodies for every
/// read AND write so the domain modules (`graph`, `ontology`, `retrieval`,
/// `snapshot`, `delta`, `thematic`) no longer contain raw SQL. The 6×
/// BEGIN/COMMIT/ROLLBACK pattern collapses to one [`run_txn`] helper.
///
/// [`run_txn`]: SqliteGraphRepo::run_txn
pub struct SqliteGraphRepo {
    db: Db,
}

impl SqliteGraphRepo {
    /// Wrap a [`Db`] handle. `Db` is `Clone` (inner `Arc`), so a production
    /// `AppState` and this adapter may share one connection cheaply.
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Run `f` inside a `BEGIN`/`COMMIT`/`ROLLBACK` transaction against the
    /// single shared connection (ADR-0001). The connection is locked for the
    /// duration of `f` via [`Db::with_conn`](crate::db::Db); `COMMIT` on `Ok`, `ROLLBACK` on `Err`.
    /// Every write trait method calls this instead of each inlining the
    /// transaction boilerplate — the 6× pattern (ingest, delete, approve-merge,
    /// ontology approve, ontology refactor, chat endorse) collapses to one
    /// helper. (#46 routes the first three through this; #47 will route the
    /// rest.)
    async fn run_txn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&rusqlite::Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        self.db
            .with_conn(move |conn| {
                conn.execute_batch("BEGIN")?;
                match f(conn) {
                    Ok(t) => {
                        conn.execute_batch("COMMIT")?;
                        Ok(t)
                    }
                    Err(e) => {
                        let _ = conn.execute_batch("ROLLBACK");
                        Err(e)
                    }
                }
            })
            .await
    }
}

#[async_trait]
impl GraphRepo for SqliteGraphRepo {
    async fn braindump_embedding_stored(&self, braindump_id: i64) -> Result<bool> {
        self.db
            .with_conn(move |conn| {
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
            .with_conn(|conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(|conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(|conn| {
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
            .with_conn(|conn| {
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
            .with_conn(move |conn| knn_concept_conn(conn, &query_vec))
            .await
    }

    async fn knn_type(&self, query_vec: &[f32]) -> Result<Option<(String, f32)>> {
        let blob = vec_to_blob(query_vec);
        self.db
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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
            .with_conn(move |conn| {
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

    // --- write paths (issue #46) ---

    async fn insert_braindump(&self, verbatim: &str, cleaned: &str) -> Result<Braindump> {
        let verbatim = verbatim.to_string();
        let cleaned = cleaned.to_string();
        self.db
            .with_conn(move |conn| {
                let created_at = now_seconds();
                conn.execute(
                    "INSERT INTO braindumps (verbatim, cleaned, created_at)
                     VALUES (?1, ?2, ?3)",
                    params![verbatim, cleaned, created_at],
                )?;
                let id = conn.last_insert_rowid();
                Ok(Braindump {
                    id,
                    verbatim,
                    cleaned,
                    created_at,
                })
            })
            .await
    }

    async fn get_braindump(&self, id: i64) -> Result<Option<Braindump>> {
        self.db
            .with_conn(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT id, verbatim, cleaned, created_at
                         FROM braindumps WHERE id = ?1",
                        params![id],
                        row_to_braindump,
                    )
                    .optional()?;
                Ok(row)
            })
            .await
    }

    async fn update_braindump(
        &self,
        id: i64,
        verbatim: String,
        cleaned: String,
    ) -> Result<Option<Braindump>> {
        self.db
            .with_conn(move |conn| {
                let updated = conn.execute(
                    "UPDATE braindumps SET verbatim = ?1, cleaned = ?2 WHERE id = ?3",
                    params![verbatim, cleaned, id],
                )?;
                if updated == 0 {
                    return Ok(None);
                }
                let row = conn.query_row(
                    "SELECT id, verbatim, cleaned, created_at
                     FROM braindumps WHERE id = ?1",
                    params![id],
                    row_to_braindump,
                )?;
                Ok(Some(row))
            })
            .await
    }

    async fn ingest_extraction(
        &self,
        braindump_id: i64,
        braindump_vec: Vec<f32>,
        extraction: ExtractionResult,
        concept_vecs: Vec<Vec<f32>>,
    ) -> Result<IngestOutcome> {
        let concepts = extraction.concepts;
        let edges = extraction.edges;
        self.run_txn(move |conn| {
            accrete_conn(
                conn,
                braindump_id,
                braindump_vec,
                concepts,
                concept_vecs,
                edges,
            )
        })
        .await
    }

    async fn delete_braindump(&self, braindump_id: i64) -> Result<bool> {
        self.run_txn(move |conn| {
            let exists = conn
                .query_row(
                    "SELECT 1 FROM braindumps WHERE id = ?1",
                    params![braindump_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                return Ok(false);
            }
            retract_extraction_conn(conn, braindump_id)?;
            let n = conn.execute(
                "DELETE FROM braindumps WHERE id = ?1",
                params![braindump_id],
            )?;
            Ok(n > 0)
        })
        .await
    }

    async fn approve_merge_suggestion(&self, suggestion_id: i64) -> Result<()> {
        self.run_txn(move |conn| {
            let pair = conn
                .query_row(
                    "SELECT new_concept_id, existing_concept_id FROM merge_suggestions
                     WHERE id = ?1",
                    params![suggestion_id],
                    |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
                )
                .optional()?;
            let Some((fold_id, keep_id)) = pair else {
                return Err(Error::NotFound(format!(
                    "merge suggestion {suggestion_id} not found"
                )));
            };
            if fold_id == keep_id {
                return Err(Error::BadRequest(
                    "merge suggestion references the same concept twice".into(),
                ));
            }
            merge_concepts_conn(conn, keep_id, fold_id)
        })
        .await
    }

    async fn reject_merge_suggestion(&self, suggestion_id: i64) -> Result<()> {
        self.db
            .with_conn(move |conn| {
                let n = conn.execute(
                    "DELETE FROM merge_suggestions WHERE id = ?1",
                    params![suggestion_id],
                )?;
                if n == 0 {
                    return Err(Error::NotFound(format!(
                        "merge suggestion {suggestion_id} not found"
                    )));
                }
                Ok(())
            })
            .await
    }

    // --- chat write-back (issue #47) ---

    async fn propose_structural_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        evidence_path: Vec<EvidenceEdge>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal> {
        let proposed_type = proposed_type.trim().to_string();
        let rationale = rationale
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let evidence_json = serde_json::to_string(&evidence_path)
            .map_err(|e| Error::internal(format!("encode evidence_path: {e}")))?;
        let created_at = now_seconds();
        self.db
            .with_conn(move |conn| {
                if !ontology_slug_exists_conn(conn, &proposed_type)? {
                    return Err(Error::BadRequest(format!(
                        "proposed type `{proposed_type}` is not in the ontology; \
                         propose it via POST /ontology/propose and re-propose the \
                         inference once approved"
                    )));
                }
                for hop in &evidence_path {
                    if !edge_exists_with_current_type_conn(
                        conn,
                        hop.source_concept_id,
                        &hop.edge_type,
                        hop.target_concept_id,
                    )? {
                        return Err(Error::BadRequest(format!(
                            "evidence path hop {} —[{}]→ {} is not a traversable edge in the graph",
                            hop.source_concept_id, hop.edge_type, hop.target_concept_id
                        )));
                    }
                }
                conn.execute(
                    "INSERT INTO chat_inference_proposals
                        (mode, source_concept_id, target_concept_id, proposed_type,
                         evidence_path, rationale, snapshot_id, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)",
                    params![
                        STRUCTURAL_MODE,
                        source_concept_id,
                        target_concept_id,
                        proposed_type,
                        evidence_json,
                        rationale,
                        STATUS_PENDING,
                        created_at
                    ],
                )?;
                Ok(ChatInferenceProposal {
                    id: conn.last_insert_rowid(),
                    mode: STRUCTURAL_MODE.to_string(),
                    source_concept_id,
                    target_concept_id,
                    proposed_type,
                    evidence_path,
                    rationale,
                    status: STATUS_PENDING.to_string(),
                    created_at,
                    resolved_at: None,
                    snapshot: None,
                })
            })
            .await
    }

    async fn propose_thematic_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        cluster_concept_ids: Vec<i64>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal> {
        let proposed_type = proposed_type.trim().to_string();
        let mut cluster = cluster_concept_ids;
        cluster.sort_unstable();
        cluster.dedup();
        let rationale = rationale
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let created_at = now_seconds();
        self.db
            .with_conn(move |conn| {
                if !ontology_slug_exists_conn(conn, &proposed_type)? {
                    return Err(Error::BadRequest(format!(
                        "proposed type `{proposed_type}` is not in the ontology; \
                         propose it via POST /ontology/propose and re-propose the \
                         inference once approved"
                    )));
                }
                for &cid in &cluster {
                    if !concept_exists_conn(conn, cid)? {
                        return Err(Error::BadRequest(format!(
                            "cluster concept id {cid} does not exist"
                        )));
                    }
                }
                let braindump_ids = compute_cluster_braindump_ids_conn(conn, &cluster)?;
                if braindump_ids.is_empty() {
                    return Err(Error::BadRequest(
                        "the motivating cluster has no braindump-backed edges between \
                         its concepts — no thematic density from user thoughts"
                            .into(),
                    ));
                }
                let braindump_json = serde_json::to_string(&braindump_ids)
                    .map_err(|e| Error::internal(format!("encode braindump_ids: {e}")))?;
                let concept_json = serde_json::to_string(&cluster)
                    .map_err(|e| Error::internal(format!("encode concept_ids: {e}")))?;
                conn.execute(
                    "INSERT INTO thematic_snapshots (braindump_ids, concept_ids, captured_at)
                     VALUES (?1, ?2, ?3)",
                    params![braindump_json, concept_json, created_at],
                )?;
                let snapshot_id = conn.last_insert_rowid();
                let evidence_json = serde_json::to_string(&Vec::<EvidenceEdge>::new())
                    .map_err(|e| Error::internal(format!("encode empty evidence_path: {e}")))?;
                conn.execute(
                    "INSERT INTO chat_inference_proposals
                        (mode, source_concept_id, target_concept_id, proposed_type,
                         evidence_path, rationale, snapshot_id, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        THEMATIC_MODE,
                        source_concept_id,
                        target_concept_id,
                        proposed_type,
                        evidence_json,
                        rationale,
                        snapshot_id,
                        STATUS_PENDING,
                        created_at
                    ],
                )?;
                Ok(ChatInferenceProposal {
                    id: conn.last_insert_rowid(),
                    mode: THEMATIC_MODE.to_string(),
                    source_concept_id,
                    target_concept_id,
                    proposed_type,
                    evidence_path: Vec::new(),
                    rationale,
                    status: STATUS_PENDING.to_string(),
                    created_at,
                    resolved_at: None,
                    snapshot: Some(ThematicSnapshot {
                        id: snapshot_id,
                        braindump_ids,
                        concept_ids: cluster,
                        captured_at: created_at,
                    }),
                })
            })
            .await
    }

    async fn endorse_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal> {
        let proposal = self
            .get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("chat inference proposal {id} not found")))?;
        if proposal.status != STATUS_PENDING {
            return Err(Error::Conflict(format!(
                "proposal {id} is `{}`, not `pending` — cannot endorse",
                proposal.status
            )));
        }
        let source = proposal.source_concept_id;
        let target = proposal.target_concept_id;
        let proposed_type = proposal.proposed_type.clone();
        let mode = proposal.mode.clone();
        let snapshot_id = proposal.snapshot.as_ref().map(|s| s.id);
        self.run_txn(move |conn| {
            let edge_id =
                if let Some(eid) = find_edge_id_conn(conn, source, &proposed_type, target)? {
                    eid
                } else {
                    let eid = insert_edge_conn(conn, source, target, &proposed_type)?;
                    init_type_history_conn(conn, eid, &proposed_type)?;
                    eid
                };
            insert_inference_provenance_conn(conn, edge_id, id, &mode, snapshot_id)?;
            transition_status_conn(conn, "chat_inference_proposals", id, STATUS_ENDORSED, false)?;
            Ok(())
        })
        .await?;
        self.get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after endorse"))
    }

    async fn reject_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal> {
        self.db
            .with_conn(move |conn| {
                transition_status_conn(conn, "chat_inference_proposals", id, STATUS_REJECTED, true)
            })
            .await?;
        self.get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after reject"))
    }

    async fn list_inference_proposals(&self) -> Result<Vec<ChatInferenceProposal>> {
        self.db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT p.id, p.mode, p.source_concept_id, p.target_concept_id,
                            p.proposed_type, p.evidence_path, p.rationale,
                            p.status, p.created_at, p.resolved_at,
                            s.id, s.braindump_ids, s.concept_ids, s.captured_at
                     FROM chat_inference_proposals p
                     LEFT JOIN thematic_snapshots s ON p.snapshot_id = s.id
                     ORDER BY p.id",
                )?;
                let rows = stmt
                    .query_map([], row_to_proposal)?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn get_inference_proposal(&self, id: i64) -> Result<Option<ChatInferenceProposal>> {
        self.db
            .with_conn(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT p.id, p.mode, p.source_concept_id, p.target_concept_id,
                                p.proposed_type, p.evidence_path, p.rationale,
                                p.status, p.created_at, p.resolved_at,
                                s.id, s.braindump_ids, s.concept_ids, s.captured_at
                         FROM chat_inference_proposals p
                         LEFT JOIN thematic_snapshots s ON p.snapshot_id = s.id
                         WHERE p.id = ?1",
                        params![id],
                        row_to_proposal,
                    )
                    .optional()?;
                Ok(row)
            })
            .await
    }

    async fn edge_inference_asserted_by(&self, edge_id: i64) -> Result<Vec<InferenceAssertion>> {
        self.db
            .with_conn(move |conn| {
                let mut stmt = conn.prepare(
                    "SELECT chat_inference_id, mode, snapshot_id FROM edge_inference_provenance
                     WHERE edge_id = ?1 ORDER BY chat_inference_id",
                )?;
                let rows = stmt
                    .query_map(params![edge_id], |r| {
                        Ok(InferenceAssertion {
                            chat_inference_id: r.get(0)?,
                            mode: r.get(1)?,
                            snapshot_id: r.get(2)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    // --- ontology governance + refactor (issue #47) ---

    async fn list_type_proposals(&self) -> Result<Vec<TypeProposal>> {
        self.db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, slug, label, description, merge_of, status,
                            near_match_slug, near_match_similarity, created_at, resolved_at
                     FROM type_proposals ORDER BY id",
                )?;
                let rows = stmt
                    .query_map([], |r| {
                        Ok(TypeProposal {
                            id: r.get(0)?,
                            slug: r.get(1)?,
                            label: r.get(2)?,
                            description: r.get(3)?,
                            merge_of: r.get(4)?,
                            status: r.get(5)?,
                            near_match_slug: r.get(6)?,
                            near_match_similarity: r.get::<_, Option<f64>>(7)?.map(|f| f as f32),
                            created_at: r.get(8)?,
                            resolved_at: r.get(9)?,
                        })
                    })?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(rows)
            })
            .await
    }

    async fn get_type_proposal(&self, id: i64) -> Result<Option<TypeProposal>> {
        self.db
            .with_conn(move |conn| {
                let row = conn
                    .query_row(
                        "SELECT id, slug, label, description, merge_of, status,
                                near_match_slug, near_match_similarity, created_at, resolved_at
                         FROM type_proposals WHERE id = ?1",
                        params![id],
                        |r| {
                            Ok(TypeProposal {
                                id: r.get(0)?,
                                slug: r.get(1)?,
                                label: r.get(2)?,
                                description: r.get(3)?,
                                merge_of: r.get(4)?,
                                status: r.get(5)?,
                                near_match_slug: r.get(6)?,
                                near_match_similarity: r
                                    .get::<_, Option<f64>>(7)?
                                    .map(|f| f as f32),
                                created_at: r.get(8)?,
                                resolved_at: r.get(9)?,
                            })
                        },
                    )
                    .optional()?;
                Ok(row)
            })
            .await
    }

    async fn insert_type_proposal(
        &self,
        slug: String,
        label: String,
        description: String,
        merge_of: Option<String>,
        status: String,
        near_match_slug: Option<String>,
        near_match_similarity: Option<f32>,
    ) -> Result<TypeProposal> {
        let created_at = now_seconds();
        self.db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT INTO type_proposals
                        (slug, label, description, merge_of, status,
                         near_match_slug, near_match_similarity, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        slug,
                        label,
                        description,
                        merge_of,
                        status,
                        near_match_slug,
                        near_match_similarity,
                        created_at
                    ],
                )?;
                Ok(TypeProposal {
                    id: conn.last_insert_rowid(),
                    slug,
                    label,
                    description,
                    merge_of,
                    status,
                    near_match_slug,
                    near_match_similarity,
                    created_at,
                    resolved_at: None,
                })
            })
            .await
    }

    async fn approve_type_proposal(
        &self,
        id: i64,
        slug: String,
        label: String,
        description: String,
        type_vec: Vec<f32>,
    ) -> Result<()> {
        let now = now_seconds();
        self.run_txn(move |conn| {
            conn.execute(
                "INSERT INTO ontology (slug, label, description, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![slug, label, description, now],
            )?;
            let ontology_id: i64 = conn.query_row(
                "SELECT id FROM ontology WHERE slug = ?1",
                params![slug],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO type_embeddings (ontology_id, embedding) VALUES (?1, ?2)",
                params![ontology_id, vec_to_blob(&type_vec)],
            )?;
            transition_status_conn(conn, "type_proposals", id, "approved", false)?;
            Ok(())
        })
        .await
    }

    async fn reject_type_proposal(&self, id: i64) -> Result<()> {
        self.db
            .with_conn(move |conn| {
                transition_status_conn(conn, "type_proposals", id, "rejected", true)
            })
            .await
    }

    async fn current_edge_type(&self, edge_id: i64) -> Result<Option<String>> {
        self.db
            .with_conn(move |conn| {
                let slug = conn
                    .query_row(
                        "SELECT type_slug FROM edge_type_history
                         WHERE edge_id = ?1 ORDER BY seq_index DESC LIMIT 1",
                        params![edge_id],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?;
                Ok(slug)
            })
            .await
    }

    async fn edges_with_current_type(&self, slug: &str) -> Result<Vec<i64>> {
        let slug = slug.to_string();
        self.db
            .with_conn(move |conn| {
                let mut stmt = conn.prepare(&format!(
                    "SELECT e.id FROM edges e WHERE ({}) = ?1 ORDER BY e.id",
                    current_type_subquery()
                ))?;
                let ids = stmt
                    .query_map(params![slug], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                Ok(ids)
            })
            .await
    }

    async fn edge_endpoints_and_type(&self, edge_id: i64) -> Result<(String, String, String)> {
        self.db
            .with_conn(move |conn| {
                let row = conn.query_row(
                    &format!(
                        "SELECT sc.label, tc.label, ({})
                         FROM edges e
                         JOIN concepts sc ON sc.id = e.source_concept_id
                         JOIN concepts tc ON tc.id = e.target_concept_id
                         WHERE e.id = ?1",
                        current_type_subquery()
                    ),
                    params![edge_id],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                        ))
                    },
                )?;
                Ok(row)
            })
            .await
    }

    async fn run_refactor(
        &self,
        llm: &dyn Llm,
        proposal: &TypeProposal,
    ) -> Result<RefactorOutcome> {
        let Some(merge_of) = proposal.merge_of.as_ref() else {
            return Ok(RefactorOutcome::default());
        };
        let edge_ids = self.edges_with_current_type(merge_of).await?;
        if edge_ids.is_empty() {
            return Ok(RefactorOutcome::default());
        }

        let ontology = self.ontology_slugs().await?;
        let new_slug = proposal.slug.clone();
        let system = "You re-classify edges when the ontology evolves. \
                      Given an edge and the new vocabulary, respond with the single slug \
                      that best fits the edge now. Respond with only the slug, nothing else.";
        let merge_of_for_prompt = merge_of.clone();
        let label_for_prompt = proposal.label.clone();
        let description_for_prompt = proposal.description.clone();

        let mut retagged: Vec<(i64, String)> = Vec::with_capacity(edge_ids.len());
        for edge_id in edge_ids {
            let (source_label, target_label, current_type) =
                self.edge_endpoints_and_type(edge_id).await?;
            let user = format!(
                "Edge: {source_label} —[{current_type}]→ {target_label}\n\
                 The type `{merge_of_for_prompt}` has been merged into `{new_slug}` \
                 (label: {label_for_prompt}; description: {description_for_prompt}).\n\
                 Re-classify this edge. Respond with exactly one slug from: [{}].",
                ontology.join(", ")
            );
            let response = llm.generate_pinned(system, &user).await?;
            let slug = response.trim().to_string();
            let chosen = if ontology.iter().any(|s| s == &slug) {
                slug
            } else {
                tracing::warn!(
                    edge_id,
                    raw = %slug,
                    "refactor LLM returned an unsanctioned slug; defaulting to the new type"
                );
                new_slug.clone()
            };
            retagged.push((edge_id, chosen));
        }

        let edges_retagged = retagged.len();
        self.run_txn(move |conn| {
            for (edge_id, slug) in &retagged {
                append_type_history_conn(conn, *edge_id, slug)?;
            }
            Ok(())
        })
        .await?;

        Ok(RefactorOutcome { edges_retagged })
    }

    // --- retrieval pipeline (issue #47) ---

    async fn retrieve(&self, query_vec: &[f32]) -> Result<RetrievalResult> {
        let candidates = self.knn_concepts(query_vec, SEED_TOP_K).await?;
        let seeds: Vec<(i64, f32)> = candidates
            .into_iter()
            .filter(|(_, sim)| *sim >= SEED_SIMILARITY_FLOOR)
            .collect();

        if seeds.is_empty() {
            return self.no_seed_fallback(query_vec).await;
        }

        let (traversed_edges, subgraph) = self
            .db
            .with_conn(move |conn| {
                let mut concept_labels: HashMap<i64, String> = HashMap::new();
                {
                    let mut stmt = conn.prepare("SELECT id, label FROM concepts")?;
                    let rows =
                        stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
                    for row in rows {
                        let (id, label) = row?;
                        concept_labels.insert(id, label);
                    }
                }
                let mut stmt = conn.prepare(&format!(
                    "SELECT e.source_concept_id, e.target_concept_id, ({}) AS current_type
                     FROM edges e",
                    current_type_subquery()
                ))?;
                let edge_rows = stmt.query_map([], |r| {
                    Ok(RetrievalEdgeInfo {
                        source_concept_id: r.get(0)?,
                        target_concept_id: r.get(1)?,
                        edge_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    })
                })?;
                let mut edges = Vec::new();
                for row in edge_rows {
                    edges.push(row?);
                }
                let (concept_hops, traversed_edges) = bfs_expand(&concept_labels, &edges, &seeds);
                let subgraph = collect_subgraph_braindumps_conn(conn, &concept_hops)?;
                Ok((traversed_edges, subgraph))
            })
            .await?;

        let backfill = self.backfill_braindumps(query_vec, &subgraph).await?;

        let mut all = subgraph;
        all.extend(backfill);
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(RetrievalResult {
            braindumps: all,
            paths: traversed_edges,
            mode: RetrievalMode::SeedThenExpand,
        })
    }

    // --- issue #48: additional reads/writes migrated from domain modules ---

    async fn all_edge_endpoints(&self) -> Result<Vec<(i64, i64)>> {
        self.db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT source_concept_id, target_concept_id FROM edges ORDER BY id",
                )?;
                let rows =
                    stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .await
    }

    async fn graph_delta(&self, since: i64) -> Result<GraphDelta> {
        self.db
            .with_conn(move |conn| {
                let added_concepts = delta_added_concepts_conn(conn, since)?;
                let added_edges = delta_added_edges_conn(conn, since)?;
                let deleted_concept_ids = delta_tombstoned_conn(conn, "concept", since)?;
                let deleted_edge_ids = delta_tombstoned_conn(conn, "edge", since)?;
                let retagged_edges = delta_retagged_edges_conn(conn, since)?;
                let cursor = now_seconds();
                Ok(GraphDelta {
                    cursor,
                    added_concepts,
                    added_edges,
                    deleted_concept_ids,
                    deleted_edge_ids,
                    retagged_edges,
                })
            })
            .await
    }

    async fn missing_type_rows(&self) -> Result<Vec<(i64, String, String, String)>> {
        self.db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT o.id, o.slug, o.label, o.description FROM ontology o
                     WHERE NOT EXISTS
                         (SELECT 1 FROM type_embeddings t WHERE t.ontology_id = o.id)
                     ORDER BY o.id",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row?);
                }
                Ok(out)
            })
            .await
    }

    async fn store_type_embedding(&self, ontology_id: i64, vec: Vec<f32>) -> Result<()> {
        self.db
            .with_conn(move |conn| {
                conn.execute(
                    "INSERT OR IGNORE INTO type_embeddings (ontology_id, embedding)
                     VALUES (?1, ?2)",
                    params![ontology_id, vec_to_blob(&vec)],
                )?;
                Ok(())
            })
            .await
    }
}

// --- issue #48: delta-sync helpers (migrated from delta.rs) ---
//
// These synchronous helpers run inside the `graph_delta` trait method's
// `with_conn` closure. Private — no domain module calls them.

fn delta_added_concepts_conn(conn: &rusqlite::Connection, since: i64) -> Result<Vec<Concept>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, created_at FROM concepts
         WHERE created_at > ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(Concept {
                id: r.get(0)?,
                label: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn delta_added_edges_conn(conn: &rusqlite::Connection, since: i64) -> Result<Vec<DeltaEdge>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id, e.source_concept_id, e.target_concept_id, e.original_type,
                e.created_at, ({}) AS current_type
         FROM edges e WHERE e.created_at > ?1 ORDER BY e.id",
        current_type_subquery()
    ))?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(DeltaEdge {
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
}

fn delta_tombstoned_conn(conn: &rusqlite::Connection, kind: &str, since: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT entity_id FROM graph_tombstones
         WHERE kind = ?1 AND created_at > ?2 ORDER BY entity_id",
    )?;
    let ids = stmt
        .query_map(params![kind, since], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

fn delta_retagged_edges_conn(conn: &rusqlite::Connection, since: i64) -> Result<Vec<RetaggedEdge>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id, e.source_concept_id, e.target_concept_id, e.original_type,
                ({}) AS current_type
         FROM edges e
         WHERE e.created_at <= ?1 AND EXISTS (
             SELECT 1 FROM edge_type_history eth
             WHERE eth.edge_id = e.id AND eth.seq_index > 0 AND eth.created_at > ?1)
         ORDER BY e.id",
        current_type_subquery()
    ))?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(RetaggedEdge {
                id: r.get(0)?,
                source_concept_id: r.get(1)?,
                target_concept_id: r.get(2)?,
                original_type: r.get(3)?,
                current_type: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Map a `rusqlite::Row` (the 4-column `SELECT id, verbatim, cleaned,
/// created_at FROM braindumps`) into a [`Braindump`]. Shared by
/// `get_braindump` and `update_braindump`.
fn row_to_braindump(row: &rusqlite::Row) -> rusqlite::Result<Braindump> {
    Ok(Braindump {
        id: row.get(0)?,
        verbatim: row.get(1)?,
        cleaned: row.get(2)?,
        created_at: row.get(3)?,
    })
}

/// Map a `rusqlite::Row` (the 14-column SELECT from
/// `chat_inference_proposals` LEFT JOIN `thematic_snapshots`) into a
/// [`ChatInferenceProposal`].
fn row_to_proposal(r: &rusqlite::Row) -> rusqlite::Result<ChatInferenceProposal> {
    let evidence_json: String = r.get(5)?;
    let evidence_path: Vec<EvidenceEdge> = serde_json::from_str(&evidence_json).unwrap_or_default();
    let snapshot = match r.get::<_, Option<i64>>(10)? {
        Some(id) => {
            let braindump_json: String = r.get(11)?;
            let concept_json: String = r.get(12)?;
            let braindump_ids: Vec<i64> = serde_json::from_str(&braindump_json).unwrap_or_default();
            let concept_ids: Vec<i64> = serde_json::from_str(&concept_json).unwrap_or_default();
            Some(ThematicSnapshot {
                id,
                braindump_ids,
                concept_ids,
                captured_at: r.get(13)?,
            })
        }
        None => None,
    };
    Ok(ChatInferenceProposal {
        id: r.get(0)?,
        mode: r.get(1)?,
        source_concept_id: r.get(2)?,
        target_concept_id: r.get(3)?,
        proposed_type: r.get(4)?,
        evidence_path,
        rationale: r.get(6)?,
        status: r.get(7)?,
        created_at: r.get(8)?,
        resolved_at: r.get(9)?,
        snapshot,
    })
}

/// Collect braindumps from the traversed subgraph: each visited concept's
/// extraction provenance (ADR-0010). Score decays with hop distance from the
/// nearest seed.
fn collect_subgraph_braindumps_conn(
    conn: &rusqlite::Connection,
    concept_hops: &HashMap<i64, usize>,
) -> Result<Vec<RetrievedBraindump>> {
    let mut best: HashMap<i64, f32> = HashMap::new();
    for (cid, hops) in concept_hops {
        let score = 1.0 / (1.0 + *hops as f32);
        let mut stmt =
            conn.prepare("SELECT braindump_id FROM concept_provenance WHERE concept_id = ?1")?;
        let ids = stmt.query_map(params![cid], |r| r.get::<_, i64>(0))?;
        for id in ids {
            let bd_id = id?;
            let entry = best.entry(bd_id).or_insert(0.0);
            if score > *entry {
                *entry = score;
            }
        }
    }

    let mut result = Vec::new();
    for (bd_id, score) in &best {
        if let Some((id, verbatim, cleaned, created_at)) = load_braindump_row_conn(conn, *bd_id)? {
            result.push(RetrievedBraindump {
                id,
                verbatim,
                cleaned,
                created_at,
                score: *score,
                source: BraindumpSource::Subgraph,
            });
        }
    }
    Ok(result)
}

/// Braindump-embedding KNN backfill for strays the graph missed (ADR-0004).
/// Returns braindumps not already in the subgraph set, scored by similarity.
impl SqliteGraphRepo {
    /// No-seed fallback (ADR-0004): braindump-vector-direct.
    async fn no_seed_fallback(&self, query_vec: &[f32]) -> Result<RetrievalResult> {
        let hits = self.knn_braindumps(query_vec, BRAINDUMP_TOP_K).await?;
        let braindumps = self
            .db
            .with_conn(move |conn| {
                let mut out = Vec::new();
                for (bd_id, sim) in &hits {
                    if let Some((id, verbatim, cleaned, created_at)) =
                        load_braindump_row_conn(conn, *bd_id)?
                    {
                        out.push(RetrievedBraindump {
                            id,
                            verbatim,
                            cleaned,
                            created_at,
                            score: *sim,
                            source: BraindumpSource::VectorDirect,
                        });
                    }
                }
                Ok(out)
            })
            .await?;
        Ok(RetrievalResult {
            braindumps,
            paths: Vec::new(),
            mode: RetrievalMode::NoSeedFallback,
        })
    }

    async fn backfill_braindumps(
        &self,
        query_vec: &[f32],
        subgraph: &[RetrievedBraindump],
    ) -> Result<Vec<RetrievedBraindump>> {
        let already: HashSet<i64> = subgraph.iter().map(|b| b.id).collect();
        let hits = self.knn_braindumps(query_vec, BRAINDUMP_TOP_K).await?;
        let backfill = self
            .db
            .with_conn(move |conn| {
                let mut out = Vec::new();
                for (bd_id, sim) in &hits {
                    if already.contains(bd_id) {
                        continue;
                    }
                    if let Some((id, verbatim, cleaned, created_at)) =
                        load_braindump_row_conn(conn, *bd_id)?
                    {
                        out.push(RetrievedBraindump {
                            id,
                            verbatim,
                            cleaned,
                            created_at,
                            score: *sim,
                            source: BraindumpSource::Backfill,
                        });
                    }
                }
                Ok(out)
            })
            .await?;
        Ok(backfill)
    }
}

// --- private accretion helpers (Sqlite adapter) ---
//
// Moved from `graph.rs` in issue #46. These run inside `run_txn` closures;
// the domain module `graph.rs` no longer calls them — its free-function
// `ingest_extraction` / `delete_braindump` wrappers delegate to the trait.

/// Run the full accretion for one braindump inside an open transaction.
fn accrete_conn(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    braindump_vec: Vec<f32>,
    concepts: Vec<ExtractedConcept>,
    concept_vecs: Vec<Vec<f32>>,
    edges: Vec<ExtractedEdge>,
) -> Result<IngestOutcome> {
    let mut outcome = IngestOutcome::default();

    retract_extraction_conn(conn, braindump_id)?;

    store_braindump_embedding_conn(conn, braindump_id, &braindump_vec)?;

    let ontology: Vec<String> = ontology_slugs_conn(conn)?;

    // Resolve each extracted concept: accrete, suggest, or create. Build a
    // label→concept_id map for the edge step (edges reference concepts by the
    // label the LLM emitted in the same extraction).
    let mut label_to_id: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut seen_labels: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (concept, vec) in concepts.iter().zip(concept_vecs.iter()) {
        if !seen_labels.insert(concept.label.as_str()) {
            continue;
        }
        let resolved = resolve_concept_conn(conn, braindump_id, &concept.label, vec)?;
        match resolved {
            ConceptResolution::Accreted(existing_id) => {
                outcome.concepts_accreted += 1;
                label_to_id.insert(concept.label.clone(), existing_id);
            }
            ConceptResolution::Created { id } => {
                outcome.concepts_created += 1;
                label_to_id.insert(concept.label.clone(), id);
            }
            ConceptResolution::Suggested { new_id } => {
                outcome.concepts_created += 1;
                outcome.merge_suggestions += 1;
                label_to_id.insert(concept.label.clone(), new_id);
            }
        }
    }

    // Edges accrete by (source, original_type, target). Unsanctioned types are
    // rejected (ADR-0002: the LLM never invents a type); edges whose endpoints
    // were not extracted as concepts in this braindump are skipped.
    let mut seen_edges: std::collections::HashSet<(&str, &str, &str)> =
        std::collections::HashSet::new();
    for edge in &edges {
        let dup_key = (
            edge.from_label.as_str(),
            edge.type_slug.as_str(),
            edge.to_label.as_str(),
        );
        if !seen_edges.insert(dup_key) {
            continue;
        }
        let Some(&source_id) = label_to_id.get(&edge.from_label) else {
            tracing::warn!(
                braindump_id,
                from = %edge.from_label,
                "edge skipped: source concept not in this extraction"
            );
            outcome.edges_rejected += 1;
            continue;
        };
        let Some(&target_id) = label_to_id.get(&edge.to_label) else {
            tracing::warn!(
                braindump_id,
                to = %edge.to_label,
                "edge skipped: target concept not in this extraction"
            );
            outcome.edges_rejected += 1;
            continue;
        };
        if !ontology.iter().any(|s| s == &edge.type_slug) {
            tracing::warn!(
                braindump_id,
                type_slug = %edge.type_slug,
                "edge rejected: type not in ontology (LLM must not invent types)"
            );
            outcome.edges_rejected += 1;
            continue;
        }
        if let Some(edge_id) = find_edge_id_conn(conn, source_id, &edge.type_slug, target_id)? {
            insert_edge_provenance_conn(conn, edge_id, braindump_id)?;
            outcome.edges_accreted += 1;
        } else {
            let edge_id = insert_edge_conn(conn, source_id, target_id, &edge.type_slug)?;
            init_type_history_conn(conn, edge_id, &edge.type_slug)?;
            insert_edge_provenance_conn(conn, edge_id, braindump_id)?;
            outcome.edges_created += 1;
        }
    }

    Ok(outcome)
}

enum ConceptResolution {
    Accreted(i64),
    Created { id: i64 },
    Suggested { new_id: i64 },
}

/// Resolve a newly-extracted concept against existing ones by embedding KNN
/// (ADR-0001). >95% accretes; borderline → new concept + merge suggestion;
/// below the floor → new concept.
fn resolve_concept_conn(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    label: &str,
    vec: &[f32],
) -> Result<ConceptResolution> {
    if let Some((existing_id, similarity)) = knn_concept_conn(conn, vec)? {
        if similarity >= ACCRETION_SIMILARITY {
            insert_concept_provenance_conn(conn, existing_id, braindump_id)?;
            return Ok(ConceptResolution::Accreted(existing_id));
        }
        if similarity >= SUGGESTION_FLOOR_SIMILARITY {
            let new_id = create_concept_conn(conn, braindump_id, label, vec)?;
            insert_merge_suggestion_conn(
                conn,
                braindump_id,
                label,
                new_id,
                existing_id,
                similarity,
            )?;
            return Ok(ConceptResolution::Suggested { new_id });
        }
    }
    let id = create_concept_conn(conn, braindump_id, label, vec)?;
    Ok(ConceptResolution::Created { id })
}

/// Create a concept, store its embedding (identity + retrieval seed), and
/// record this braindump as its first extractor (ADR-0010).
fn create_concept_conn(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    label: &str,
    vec: &[f32],
) -> Result<i64> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO concepts (label, created_at) VALUES (?1, ?2)",
        params![label, created_at],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO concept_embeddings (concept_id, embedding) VALUES (?1, ?2)",
        params![id, vec_to_blob(vec)],
    )?;
    insert_concept_provenance_conn(conn, id, braindump_id)?;
    Ok(id)
}

fn store_braindump_embedding_conn(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    vec: &[f32],
) -> Result<()> {
    conn.execute(
        "INSERT INTO braindump_embeddings (braindump_id, embedding) VALUES (?1, ?2)",
        params![braindump_id, vec_to_blob(vec)],
    )?;
    Ok(())
}

fn insert_concept_provenance_conn(
    conn: &rusqlite::Connection,
    concept_id: i64,
    braindump_id: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO concept_provenance (concept_id, braindump_id) VALUES (?1, ?2)",
        params![concept_id, braindump_id],
    )?;
    Ok(())
}

fn insert_edge_provenance_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    braindump_id: i64,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO edge_provenance (edge_id, braindump_id) VALUES (?1, ?2)",
        params![edge_id, braindump_id],
    )?;
    Ok(())
}

fn insert_merge_suggestion_conn(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    new_label: &str,
    new_concept_id: i64,
    existing_concept_id: i64,
    similarity: f32,
) -> Result<()> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO merge_suggestions
            (kind, braindump_id, new_concept_label, new_concept_id, existing_concept_id,
             similarity, status, created_at)
         VALUES ('concept', ?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
        params![
            braindump_id,
            new_label,
            new_concept_id,
            existing_concept_id,
            similarity,
            created_at
        ],
    )?;
    Ok(())
}

/// Retract a braindump's prior extraction (idempotent over a braindump, so
/// submit retracts nothing and edit retracts the stale extraction before
/// re-accreting — ADR-0007). Concepts/edges that lose their last asserter
/// vanish (ADR-0002 / ADR-0010); type-history and suggestions cascade. Vanished
/// concepts/edges are tombstoned into `graph_tombstones` before the row DELETEs
/// so delta sync can report what disappeared (issue #28).
fn retract_extraction_conn(conn: &rusqlite::Connection, braindump_id: i64) -> Result<()> {
    conn.execute(
        "DELETE FROM concept_provenance WHERE braindump_id = ?1",
        params![braindump_id],
    )?;
    conn.execute(
        "DELETE FROM edge_provenance WHERE braindump_id = ?1",
        params![braindump_id],
    )?;
    conn.execute(
        "DELETE FROM braindump_embeddings WHERE braindump_id = ?1",
        params![braindump_id],
    )?;
    conn.execute(
        "DELETE FROM merge_suggestions WHERE braindump_id = ?1",
        params![braindump_id],
    )?;
    // Tombstone orphan edges (no asserter left) before the row DELETE so delta
    // sync can report the deletion (issue #28). An edge's asserter list is the
    // union of braindump provenance (`edge_provenance`, ADR-0002) and
    // chat-inference provenance (`edge_inference_provenance`, ADR-0006) — so an
    // edge backed only by a chat inference is NOT orphaned by a braindump
    // deletion (the inference is its own origin).
    tombstone_orphan_edges_conn(conn)?;
    // Orphan edges first (they reference concepts), then orphan concepts.
    conn.execute(
        "DELETE FROM edges WHERE NOT EXISTS
            (SELECT 1 FROM edge_provenance WHERE edge_id = edges.id)
          AND NOT EXISTS
            (SELECT 1 FROM edge_inference_provenance WHERE edge_id = edges.id)",
        [],
    )?;
    // The vec0 concept_embeddings table has no FK cascade — clean embeddings for
    // concepts about to vanish, so KNN never returns a deleted concept's vector.
    conn.execute(
        "DELETE FROM concept_embeddings WHERE concept_id IN
            (SELECT id FROM concepts WHERE NOT EXISTS
                (SELECT 1 FROM concept_provenance WHERE concept_id = concepts.id))",
        [],
    )?;
    // Tombstone orphan concepts (no extractor left) before the row DELETE.
    tombstone_orphan_concepts_conn(conn)?;
    conn.execute(
        "DELETE FROM concepts WHERE NOT EXISTS
            (SELECT 1 FROM concept_provenance WHERE concept_id = concepts.id)",
        [],
    )?;
    Ok(())
}

/// Append 'edge' tombstone rows for every edge about to vanish (no asserter
/// remains) in the current transaction. A single `INSERT ... SELECT` — no Rust
/// loop.
fn tombstone_orphan_edges_conn(conn: &rusqlite::Connection) -> Result<()> {
    let now = now_seconds();
    conn.execute(
        "INSERT INTO graph_tombstones (kind, entity_id, created_at)
         SELECT 'edge', id, ?1 FROM edges
         WHERE NOT EXISTS
            (SELECT 1 FROM edge_provenance WHERE edge_id = edges.id)
           AND NOT EXISTS
            (SELECT 1 FROM edge_inference_provenance WHERE edge_id = edges.id)",
        params![now],
    )?;
    Ok(())
}

/// Append 'concept' tombstone rows for every concept about to vanish (no
/// extractor remains) in the current transaction. Symmetric to
/// [`tombstone_orphan_edges_conn`].
fn tombstone_orphan_concepts_conn(conn: &rusqlite::Connection) -> Result<()> {
    let now = now_seconds();
    conn.execute(
        "INSERT INTO graph_tombstones (kind, entity_id, created_at)
         SELECT 'concept', id, ?1 FROM concepts
         WHERE NOT EXISTS
            (SELECT 1 FROM concept_provenance WHERE concept_id = concepts.id)",
        params![now],
    )?;
    Ok(())
}

/// In-memory adapter for tests: holds the graph state in HashMaps so every
/// read AND write can be exercised without a SQLite connection. KNN is
/// brute-force cosine (the test paths use small N so the cost is fine). Gate
/// on `test` and the forward-looking `test-support` feature so
/// integration-test crates (in `backend/tests/`) can enable it.
///
/// Mutators (`add_concept`, `add_edge`, `set_concept_embedding`, …) are test
/// infrastructure: tests populate state directly without standing up the
/// accretion pipeline. The write-path trait methods (issue #46:
/// `insert_braindump`, `ingest_extraction`, `delete_braindump`,
/// `approve_merge_suggestion`, `reject_merge_suggestion`) are the real
/// mutators that mirror the Sqlite adapter's semantics against the HashMaps.
#[cfg(any(test, feature = "test-support"))]
pub struct InMemoryGraphRepo {
    /// `braindump_id → Braindump` (the full braindump row set). Needed so
    /// `delete_braindump` can check existence and the cascade can fire.
    braindumps: std::sync::Mutex<std::collections::HashMap<i64, Braindump>>,
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
    /// Chat-inference proposals (ADR-0006), ordered by id.
    chat_inference_proposals: std::sync::Mutex<Vec<ChatInferenceProposal>>,
    /// `snapshot_id → ThematicSnapshot` (ADR-0009 frozen receipts).
    thematic_snapshots: std::sync::Mutex<std::collections::HashMap<i64, ThematicSnapshot>>,
    /// `edge_id → Vec<InferenceAssertion>` (ADR-0006 origin-typed provenance).
    edge_inference_provenance:
        std::sync::Mutex<std::collections::HashMap<i64, Vec<InferenceAssertion>>>,
    /// Type proposals (ADR-0003 governance queue), ordered by id.
    type_proposals: std::sync::Mutex<Vec<TypeProposal>>,
    /// Monotonic id counters for auto-generated rows (avoid id reuse across
    /// retract + re-create cycles, matching Sqlite `AUTOINCREMENT`).
    next_braindump_id: std::sync::Mutex<i64>,
    next_concept_id: std::sync::Mutex<i64>,
    next_edge_id: std::sync::Mutex<i64>,
    next_suggestion_id: std::sync::Mutex<i64>,
    next_proposal_id: std::sync::Mutex<i64>,
    next_snapshot_id: std::sync::Mutex<i64>,
    next_type_proposal_id: std::sync::Mutex<i64>,
}

#[cfg(any(test, feature = "test-support"))]
impl InMemoryGraphRepo {
    /// A fresh, empty in-memory graph. Every read returns empty/`None` until
    /// state is populated via the mutators or the write trait methods.
    pub fn new() -> Self {
        Self {
            braindumps: std::sync::Mutex::new(std::collections::HashMap::new()),
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
            chat_inference_proposals: std::sync::Mutex::new(Vec::new()),
            thematic_snapshots: std::sync::Mutex::new(std::collections::HashMap::new()),
            edge_inference_provenance: std::sync::Mutex::new(std::collections::HashMap::new()),
            type_proposals: std::sync::Mutex::new(Vec::new()),
            next_braindump_id: std::sync::Mutex::new(0),
            next_concept_id: std::sync::Mutex::new(0),
            next_edge_id: std::sync::Mutex::new(0),
            next_suggestion_id: std::sync::Mutex::new(0),
            next_proposal_id: std::sync::Mutex::new(0),
            next_snapshot_id: std::sync::Mutex::new(0),
            next_type_proposal_id: std::sync::Mutex::new(0),
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

    // --- write paths (issue #46) ---

    async fn insert_braindump(&self, verbatim: &str, cleaned: &str) -> Result<Braindump> {
        let id = Self::next_id(&self.next_braindump_id);
        let created_at = now_seconds();
        let braindump = Braindump {
            id,
            verbatim: verbatim.to_string(),
            cleaned: cleaned.to_string(),
            created_at,
        };
        self.braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(id, braindump.clone());
        Ok(braindump)
    }

    async fn ingest_extraction(
        &self,
        braindump_id: i64,
        braindump_vec: Vec<f32>,
        extraction: ExtractionResult,
        concept_vecs: Vec<Vec<f32>>,
    ) -> Result<IngestOutcome> {
        let mut outcome = IngestOutcome::default();

        self.retract_extraction_in_memory(braindump_id);

        self.braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(braindump_id, braindump_vec);

        let ontology: Vec<String> = self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .map(|(slug, _, _)| slug.clone())
            .collect();

        // Resolve each extracted concept: accrete, suggest, or create. Build a
        // label→concept_id map for the edge step.
        let mut label_to_id: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut seen_labels: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (concept, vec) in extraction.concepts.iter().zip(concept_vecs.iter()) {
            if !seen_labels.insert(concept.label.as_str()) {
                continue;
            }
            match self.resolve_concept_in_memory(braindump_id, &concept.label, vec) {
                InMemoryResolution::Accreted(existing_id) => {
                    outcome.concepts_accreted += 1;
                    label_to_id.insert(concept.label.clone(), existing_id);
                }
                InMemoryResolution::Created { id } => {
                    outcome.concepts_created += 1;
                    label_to_id.insert(concept.label.clone(), id);
                }
                InMemoryResolution::Suggested { new_id } => {
                    outcome.concepts_created += 1;
                    outcome.merge_suggestions += 1;
                    label_to_id.insert(concept.label.clone(), new_id);
                }
            }
        }

        // Edges accrete by (source, original_type, target). Unsanctioned types
        // are rejected; edges whose endpoints were not extracted in this
        // braindump are skipped.
        let mut seen_edges: std::collections::HashSet<(&str, &str, &str)> =
            std::collections::HashSet::new();
        for edge in &extraction.edges {
            let dup_key = (
                edge.from_label.as_str(),
                edge.type_slug.as_str(),
                edge.to_label.as_str(),
            );
            if !seen_edges.insert(dup_key) {
                continue;
            }
            let Some(&source_id) = label_to_id.get(&edge.from_label) else {
                outcome.edges_rejected += 1;
                continue;
            };
            let Some(&target_id) = label_to_id.get(&edge.to_label) else {
                outcome.edges_rejected += 1;
                continue;
            };
            if !ontology.iter().any(|s| s == &edge.type_slug) {
                outcome.edges_rejected += 1;
                continue;
            }
            let existing_edge_id = self
                .edges
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .values()
                .find(|e| {
                    e.source_concept_id == source_id
                        && e.original_type == edge.type_slug
                        && e.target_concept_id == target_id
                })
                .map(|e| e.id);
            if let Some(edge_id) = existing_edge_id {
                self.add_edge_provenance(edge_id, braindump_id);
                outcome.edges_accreted += 1;
            } else {
                let edge_id = self.create_edge_in_memory(source_id, target_id, &edge.type_slug);
                self.add_edge_provenance(edge_id, braindump_id);
                outcome.edges_created += 1;
            }
        }

        Ok(outcome)
    }

    async fn delete_braindump(&self, braindump_id: i64) -> Result<bool> {
        let exists = self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .contains_key(&braindump_id);
        if !exists {
            return Ok(false);
        }
        self.retract_extraction_in_memory(braindump_id);
        self.braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .remove(&braindump_id);
        Ok(true)
    }

    async fn approve_merge_suggestion(&self, suggestion_id: i64) -> Result<()> {
        let (fold_id, keep_id) = {
            let suggestions = self
                .merge_suggestions
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            let s = suggestions
                .iter()
                .find(|s| s.id == suggestion_id)
                .ok_or_else(|| {
                    Error::NotFound(format!("merge suggestion {suggestion_id} not found"))
                })?;
            (s.new_concept_id, s.existing_concept_id)
        };
        if fold_id == keep_id {
            return Err(Error::BadRequest(
                "merge suggestion references the same concept twice".into(),
            ));
        }
        self.merge_concepts_in_memory(keep_id, fold_id);
        Ok(())
    }

    async fn reject_merge_suggestion(&self, suggestion_id: i64) -> Result<()> {
        let mut suggestions = self
            .merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let before = suggestions.len();
        suggestions.retain(|s| s.id != suggestion_id);
        if suggestions.len() == before {
            return Err(Error::NotFound(format!(
                "merge suggestion {suggestion_id} not found"
            )));
        }
        Ok(())
    }

    // --- chat write-back (issue #47) ---

    async fn propose_structural_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        evidence_path: Vec<EvidenceEdge>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal> {
        let proposed_type = proposed_type.trim().to_string();
        let rationale = rationale
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let ontology = self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .any(|(slug, _, _)| slug == &proposed_type);
        if !ontology {
            return Err(Error::BadRequest(format!(
                "proposed type `{proposed_type}` is not in the ontology; \
                 propose it via POST /ontology/propose and re-propose the \
                 inference once approved"
            )));
        }
        for hop in &evidence_path {
            if !self.edge_exists_with_current_type_in_memory(
                hop.source_concept_id,
                &hop.edge_type,
                hop.target_concept_id,
            ) {
                return Err(Error::BadRequest(format!(
                    "evidence path hop {} —[{}]→ {} is not a traversable edge in the graph",
                    hop.source_concept_id, hop.edge_type, hop.target_concept_id
                )));
            }
        }
        let id = Self::next_id(&self.next_proposal_id);
        let created_at = now_seconds();
        let proposal = ChatInferenceProposal {
            id,
            mode: STRUCTURAL_MODE.to_string(),
            source_concept_id,
            target_concept_id,
            proposed_type,
            evidence_path,
            rationale,
            status: STATUS_PENDING.to_string(),
            created_at,
            resolved_at: None,
            snapshot: None,
        };
        self.chat_inference_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push(proposal.clone());
        Ok(proposal)
    }

    async fn propose_thematic_inference(
        &self,
        source_concept_id: i64,
        target_concept_id: i64,
        proposed_type: &str,
        cluster_concept_ids: Vec<i64>,
        rationale: Option<&str>,
    ) -> Result<ChatInferenceProposal> {
        let proposed_type = proposed_type.trim().to_string();
        let mut cluster = cluster_concept_ids;
        cluster.sort_unstable();
        cluster.dedup();
        let rationale = rationale
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        let ontology = self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .any(|(slug, _, _)| slug == &proposed_type);
        if !ontology {
            return Err(Error::BadRequest(format!(
                "proposed type `{proposed_type}` is not in the ontology; \
                 propose it via POST /ontology/propose and re-propose the \
                 inference once approved"
            )));
        }
        let concepts_exist: bool = {
            let concepts = self
                .concepts
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            cluster.iter().all(|cid| concepts.contains_key(cid))
        };
        if !concepts_exist {
            let missing = cluster.iter().find(|cid| {
                !self
                    .concepts
                    .lock()
                    .expect("InMemoryGraphRepo mutex poisoned")
                    .contains_key(cid)
            });
            if let Some(cid) = missing {
                return Err(Error::BadRequest(format!(
                    "cluster concept id {cid} does not exist"
                )));
            }
        }
        let braindump_ids = self.compute_cluster_braindump_ids_in_memory(&cluster);
        if braindump_ids.is_empty() {
            return Err(Error::BadRequest(
                "the motivating cluster has no braindump-backed edges between \
                 its concepts — no thematic density from user thoughts"
                    .into(),
            ));
        }
        let snapshot_id = Self::next_id(&self.next_snapshot_id);
        let snapshot = ThematicSnapshot {
            id: snapshot_id,
            braindump_ids: braindump_ids.clone(),
            concept_ids: cluster.clone(),
            captured_at: now_seconds(),
        };
        self.thematic_snapshots
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(snapshot_id, snapshot.clone());
        let id = Self::next_id(&self.next_proposal_id);
        let created_at = now_seconds();
        let proposal = ChatInferenceProposal {
            id,
            mode: THEMATIC_MODE.to_string(),
            source_concept_id,
            target_concept_id,
            proposed_type,
            evidence_path: Vec::new(),
            rationale,
            status: STATUS_PENDING.to_string(),
            created_at,
            resolved_at: None,
            snapshot: Some(snapshot),
        };
        self.chat_inference_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push(proposal.clone());
        Ok(proposal)
    }

    async fn endorse_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal> {
        let proposal = self
            .get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::NotFound(format!("chat inference proposal {id} not found")))?;
        if proposal.status != STATUS_PENDING {
            return Err(Error::Conflict(format!(
                "proposal {id} is `{}`, not `pending` — cannot endorse",
                proposal.status
            )));
        }
        let source = proposal.source_concept_id;
        let target = proposal.target_concept_id;
        let proposed_type = proposal.proposed_type.clone();
        let mode = proposal.mode.clone();
        let snapshot_id = proposal.snapshot.as_ref().map(|s| s.id);

        let edge_id = {
            let edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
            let existing = edges
                .values()
                .find(|e| {
                    e.source_concept_id == source
                        && e.original_type == proposed_type
                        && e.target_concept_id == target
                })
                .map(|e| e.id);
            drop(edges);
            match existing {
                Some(eid) => eid,
                None => self.create_edge_in_memory(source, target, &proposed_type),
            }
        };
        {
            let mut prov = self
                .edge_inference_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            let entries = prov.entry(edge_id).or_default();
            if !entries.iter().any(|a| a.chat_inference_id == id) {
                entries.push(InferenceAssertion {
                    chat_inference_id: id,
                    mode,
                    snapshot_id,
                });
            }
        }
        self.transition_proposal_status_in_memory(id, STATUS_ENDORSED);
        self.get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after endorse"))
    }

    async fn reject_inference_proposal(&self, id: i64) -> Result<ChatInferenceProposal> {
        {
            let proposals = self
                .chat_inference_proposals
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            let p = proposals.iter().find(|p| p.id == id);
            match p {
                None => {
                    return Err(Error::NotFound(format!(
                        "chat inference proposal {id} not found"
                    )));
                }
                Some(p) if p.status != STATUS_PENDING => {
                    return Err(Error::Conflict(format!(
                        "proposal {id} is `{}`, not `pending` — cannot reject",
                        p.status
                    )));
                }
                _ => {}
            }
        }
        self.transition_proposal_status_in_memory(id, STATUS_REJECTED);
        self.get_inference_proposal(id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after reject"))
    }

    async fn list_inference_proposals(&self) -> Result<Vec<ChatInferenceProposal>> {
        Ok(self
            .chat_inference_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone())
    }

    async fn get_inference_proposal(&self, id: i64) -> Result<Option<ChatInferenceProposal>> {
        Ok(self
            .chat_inference_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    async fn edge_inference_asserted_by(&self, edge_id: i64) -> Result<Vec<InferenceAssertion>> {
        Ok(self
            .edge_inference_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&edge_id)
            .cloned()
            .unwrap_or_default())
    }

    // --- ontology governance + refactor (issue #47) ---

    async fn list_type_proposals(&self) -> Result<Vec<TypeProposal>> {
        Ok(self
            .type_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone())
    }

    async fn get_type_proposal(&self, id: i64) -> Result<Option<TypeProposal>> {
        Ok(self
            .type_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .find(|p| p.id == id)
            .cloned())
    }

    async fn insert_type_proposal(
        &self,
        slug: String,
        label: String,
        description: String,
        merge_of: Option<String>,
        status: String,
        near_match_slug: Option<String>,
        near_match_similarity: Option<f32>,
    ) -> Result<TypeProposal> {
        let id = Self::next_id(&self.next_type_proposal_id);
        let created_at = now_seconds();
        let proposal = TypeProposal {
            id,
            slug,
            label,
            description,
            merge_of,
            status,
            near_match_slug,
            near_match_similarity,
            created_at,
            resolved_at: None,
        };
        self.type_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push(proposal.clone());
        Ok(proposal)
    }

    async fn approve_type_proposal(
        &self,
        id: i64,
        slug: String,
        label: String,
        description: String,
        type_vec: Vec<f32>,
    ) -> Result<()> {
        {
            let proposals = self
                .type_proposals
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            let p = proposals.iter().find(|p| p.id == id);
            match p {
                None => {
                    return Err(Error::NotFound(format!("type proposal {id} not found")));
                }
                Some(p) if p.status != "pending" => {
                    return Err(Error::Conflict(format!(
                        "proposal {id} is `{}`, not `pending` — cannot approve",
                        p.status
                    )));
                }
                _ => {}
            }
        }
        let now = now_seconds();
        self.ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push((slug.clone(), label.clone(), description.clone()));
        self.set_type_embedding(&slug, type_vec);
        {
            let mut proposals = self
                .type_proposals
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            if let Some(p) = proposals.iter_mut().find(|p| p.id == id) {
                p.status = "approved".to_string();
                p.resolved_at = Some(now);
            }
        }
        Ok(())
    }

    async fn reject_type_proposal(&self, id: i64) -> Result<()> {
        {
            let proposals = self
                .type_proposals
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            let p = proposals.iter().find(|p| p.id == id);
            match p {
                None => {
                    return Err(Error::NotFound(format!("type proposal {id} not found")));
                }
                Some(p) if p.status != "pending" => {
                    return Err(Error::Conflict(format!(
                        "proposal {id} is `{}`, not `pending` — cannot reject",
                        p.status
                    )));
                }
                _ => {}
            }
        }
        let now = now_seconds();
        {
            let mut proposals = self
                .type_proposals
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            if let Some(p) = proposals.iter_mut().find(|p| p.id == id) {
                p.status = "rejected".to_string();
                p.resolved_at = Some(now);
            }
        }
        Ok(())
    }

    async fn current_edge_type(&self, edge_id: i64) -> Result<Option<String>> {
        Ok(self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&edge_id)
            .and_then(|entries| {
                entries
                    .iter()
                    .max_by_key(|h| h.seq_index)
                    .map(|h| h.type_slug.clone())
            }))
    }

    async fn edges_with_current_type(&self, slug: &str) -> Result<Vec<i64>> {
        let slug = slug.to_string();
        let history = self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let mut ids: Vec<i64> = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .filter(|e| {
                history
                    .get(&e.id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .max_by_key(|h| h.seq_index)
                            .map(|h| h.type_slug == slug)
                    })
                    .unwrap_or(false)
            })
            .map(|e| e.id)
            .collect();
        ids.sort_unstable();
        Ok(ids)
    }

    async fn edge_endpoints_and_type(&self, edge_id: i64) -> Result<(String, String, String)> {
        let (source_concept_id, target_concept_id) = {
            let edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
            let edge = edges
                .get(&edge_id)
                .ok_or_else(|| Error::NotFound(format!("edge {edge_id} not found")))?;
            (edge.source_concept_id, edge.target_concept_id)
        };
        let source_label = self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&source_concept_id)
            .map(|c| c.label.clone())
            .unwrap_or_default();
        let target_label = self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&target_concept_id)
            .map(|c| c.label.clone())
            .unwrap_or_default();
        let current_type = self.current_edge_type(edge_id).await?.unwrap_or_default();
        Ok((source_label, target_label, current_type))
    }

    async fn run_refactor(
        &self,
        llm: &dyn Llm,
        proposal: &TypeProposal,
    ) -> Result<RefactorOutcome> {
        let Some(merge_of) = proposal.merge_of.as_ref() else {
            return Ok(RefactorOutcome::default());
        };
        let edge_ids = self.edges_with_current_type(merge_of).await?;
        if edge_ids.is_empty() {
            return Ok(RefactorOutcome::default());
        }

        let ontology = self.ontology_slugs().await?;
        let new_slug = proposal.slug.clone();
        let system = "You re-classify edges when the ontology evolves. \
                      Given an edge and the new vocabulary, respond with the single slug \
                      that best fits the edge now. Respond with only the slug, nothing else.";
        let merge_of_for_prompt = merge_of.clone();
        let label_for_prompt = proposal.label.clone();
        let description_for_prompt = proposal.description.clone();

        let mut retagged: Vec<(i64, String)> = Vec::with_capacity(edge_ids.len());
        for edge_id in edge_ids {
            let (source_label, target_label, current_type) =
                self.edge_endpoints_and_type(edge_id).await?;
            let user = format!(
                "Edge: {source_label} —[{current_type}]→ {target_label}\n\
                 The type `{merge_of_for_prompt}` has been merged into `{new_slug}` \
                 (label: {label_for_prompt}; description: {description_for_prompt}).\n\
                 Re-classify this edge. Respond with exactly one slug from: [{}].",
                ontology.join(", ")
            );
            let response = llm.generate_pinned(system, &user).await?;
            let slug = response.trim().to_string();
            let chosen = if ontology.iter().any(|s| s == &slug) {
                slug
            } else {
                tracing::warn!(
                    edge_id,
                    raw = %slug,
                    "refactor LLM returned an unsanctioned slug; defaulting to the new type"
                );
                new_slug.clone()
            };
            retagged.push((edge_id, chosen));
        }

        let edges_retagged = retagged.len();
        let now = now_seconds();
        for (edge_id, slug) in &retagged {
            self.append_edge_type_history(*edge_id, slug, now);
        }

        Ok(RefactorOutcome { edges_retagged })
    }

    // --- retrieval pipeline (issue #47) ---

    async fn retrieve(&self, query_vec: &[f32]) -> Result<RetrievalResult> {
        let candidates = self.knn_concepts(query_vec, SEED_TOP_K).await?;
        let seeds: Vec<(i64, f32)> = candidates
            .into_iter()
            .filter(|(_, sim)| *sim >= SEED_SIMILARITY_FLOOR)
            .collect();

        if seeds.is_empty() {
            return self.no_seed_fallback_in_memory(query_vec).await;
        }

        let concept_labels: HashMap<i64, String> = self
            .concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .map(|(id, c)| (*id, c.label.clone()))
            .collect();
        let history = self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let edges: Vec<RetrievalEdgeInfo> = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .map(|e| RetrievalEdgeInfo {
                edge_type: history
                    .get(&e.id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .max_by_key(|h| h.seq_index)
                            .map(|h| h.type_slug.clone())
                    })
                    .unwrap_or_default(),
                source_concept_id: e.source_concept_id,
                target_concept_id: e.target_concept_id,
            })
            .collect();
        let (concept_hops, traversed_edges) = bfs_expand(&concept_labels, &edges, &seeds);
        let subgraph = self.collect_subgraph_braindumps_in_memory(&concept_hops);
        let backfill = self
            .backfill_braindumps_in_memory(query_vec, &subgraph)
            .await;

        let mut all = subgraph;
        all.extend(backfill);
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(RetrievalResult {
            braindumps: all,
            paths: traversed_edges,
            mode: RetrievalMode::SeedThenExpand,
        })
    }

    // --- issue #48: additional reads/writes migrated from domain modules ---

    async fn get_braindump(&self, id: i64) -> Result<Option<Braindump>> {
        Ok(self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&id)
            .cloned())
    }

    async fn update_braindump(
        &self,
        id: i64,
        verbatim: String,
        cleaned: String,
    ) -> Result<Option<Braindump>> {
        let mut braindumps = self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let Some(braindump) = braindumps.get_mut(&id) else {
            return Ok(None);
        };
        braindump.verbatim = verbatim;
        braindump.cleaned = cleaned;
        Ok(Some(braindump.clone()))
    }

    async fn all_edge_endpoints(&self) -> Result<Vec<(i64, i64)>> {
        let mut endpoints: Vec<(i64, i64)> = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .map(|e| (e.source_concept_id, e.target_concept_id))
            .collect();
        endpoints.sort_by_key(|(src, _)| *src);
        Ok(endpoints)
    }

    async fn graph_delta(&self, since: i64) -> Result<GraphDelta> {
        let added_concepts: Vec<Concept> = self
            .all_concepts()
            .await?
            .into_iter()
            .filter(|c| c.created_at > since)
            .collect();
        let added_edges: Vec<DeltaEdge> = self
            .all_edges_with_current_type()
            .await?
            .into_iter()
            .filter(|e| e.created_at > since)
            .map(|e| DeltaEdge {
                id: e.id,
                source_concept_id: e.source_concept_id,
                target_concept_id: e.target_concept_id,
                original_type: e.original_type,
                current_type: e.current_type,
                created_at: e.created_at,
            })
            .collect();
        // InMemoryGraphRepo does not track tombstones — deletions are empty.
        let deleted_concept_ids = Vec::new();
        let deleted_edge_ids = Vec::new();
        let retagged_edges = self.compute_retagged_edges_in_memory(since).await;
        Ok(GraphDelta {
            cursor: now_seconds(),
            added_concepts,
            added_edges,
            deleted_concept_ids,
            deleted_edge_ids,
            retagged_edges,
        })
    }

    async fn missing_type_rows(&self) -> Result<Vec<(i64, String, String, String)>> {
        let ontology = self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let embedded_slugs: std::collections::HashSet<String> = self
            .type_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .iter()
            .map(|(slug, _)| slug.clone())
            .collect();
        let mut out = Vec::new();
        for (idx, (slug, label, description)) in ontology.iter().enumerate() {
            if !embedded_slugs.contains(slug) {
                out.push((idx as i64, slug.clone(), label.clone(), description.clone()));
            }
        }
        Ok(out)
    }

    async fn store_type_embedding(&self, ontology_id: i64, vec: Vec<f32>) -> Result<()> {
        let ontology = self
            .ontology
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        if let Some((slug, _, _)) = ontology.get(ontology_id as usize) {
            self.set_type_embedding(slug, vec);
        }
        Ok(())
    }
}

// --- InMemoryGraphRepo private write helpers (issue #46) ---

#[cfg(any(test, feature = "test-support"))]
impl InMemoryGraphRepo {
    /// Increment-and-get from a monotonic id counter.
    fn next_id(counter: &std::sync::Mutex<i64>) -> i64 {
        let mut id = counter.lock().expect("InMemoryGraphRepo mutex poisoned");
        *id += 1;
        *id
    }

    /// Retract a braindump's prior extraction (idempotent over a braindump,
    /// mirroring [`retract_extraction_conn`] in the Sqlite adapter). Drops
    /// provenance for this braindump, vanishes orphan edges (no asserter left)
    /// and orphan concepts (no extractor left), and cleans embeddings for
    /// vanished concepts. Merge suggestions for this braindump are dropped
    /// (the cascade). Edges whose endpoint concept vanishes are cascade-deleted
    /// (ADR-0010 addendum).
    fn retract_extraction_in_memory(&self, braindump_id: i64) {
        // Drop concept provenance for this braindump.
        {
            let mut prov = self
                .concept_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            for entries in prov.values_mut() {
                entries.retain(|bd| *bd != braindump_id);
            }
        }
        // Drop edge provenance for this braindump.
        {
            let mut prov = self
                .edge_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            for entries in prov.values_mut() {
                entries.retain(|bd| *bd != braindump_id);
            }
        }
        // Drop braindump embedding.
        self.braindump_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .remove(&braindump_id);
        // Drop merge suggestions for this braindump.
        self.merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .retain(|s| s.braindump_id != braindump_id);

        // Vanish orphan edges (no asserter left). The InMemoryGraphRepo does
        // not model `edge_inference_provenance` (chat-inference provenance is
        // #47's scope), so an edge's asserter list is `edge_provenance` only.
        let orphan_edge_ids: Vec<i64> = {
            let prov = self
                .edge_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            self.edges
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .values()
                .filter(|e| prov.get(&e.id).is_none_or(|v| v.is_empty()))
                .map(|e| e.id)
                .collect()
        };
        for eid in &orphan_edge_ids {
            self.edges
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
            self.edge_type_history
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
            self.edge_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
        }

        // Vanish orphan concepts (no extractor left) + clean their embeddings.
        let orphan_concept_ids: Vec<i64> = {
            let prov = self
                .concept_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned");
            self.concepts
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .values()
                .filter(|c| prov.get(&c.id).is_none_or(|v| v.is_empty()))
                .map(|c| c.id)
                .collect()
        };
        for cid in &orphan_concept_ids {
            self.concepts
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(cid);
            self.concept_embeddings
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(cid);
            self.concept_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(cid);
        }

        // Cascade-delete edges whose endpoint concept vanished (ADR-0010
        // addendum): an edge with a missing endpoint is meaningless, even if
        // it still has an asserter.
        let orphan_concept_set: std::collections::HashSet<i64> =
            orphan_concept_ids.into_iter().collect();
        let cascade_edge_ids: Vec<i64> = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .filter(|e| {
                orphan_concept_set.contains(&e.source_concept_id)
                    || orphan_concept_set.contains(&e.target_concept_id)
            })
            .map(|e| e.id)
            .collect();
        for eid in &cascade_edge_ids {
            self.edges
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
            self.edge_type_history
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
            self.edge_provenance
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .remove(eid);
        }
    }

    /// Resolve a newly-extracted concept by brute-force cosine KNN
    /// (ADR-0001). ≥95% accretes; borderline [0.80, 0.95) → new concept +
    /// merge suggestion; below the floor → new concept.
    fn resolve_concept_in_memory(
        &self,
        braindump_id: i64,
        label: &str,
        vec: &[f32],
    ) -> InMemoryResolution {
        let embeddings = self
            .concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let best = embeddings
            .into_iter()
            .map(|(id, v)| (id, cosine_similarity(vec, &v)))
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .filter(|(_, sim)| *sim > 0.0);

        if let Some((existing_id, similarity)) = best {
            if similarity >= ACCRETION_SIMILARITY {
                self.add_concept_provenance(existing_id, braindump_id);
                return InMemoryResolution::Accreted(existing_id);
            }
            if similarity >= SUGGESTION_FLOOR_SIMILARITY {
                let new_id = self.create_concept_in_memory(braindump_id, label, vec);
                self.insert_merge_suggestion_in_memory(
                    braindump_id,
                    label,
                    new_id,
                    existing_id,
                    similarity,
                );
                return InMemoryResolution::Suggested { new_id };
            }
        }
        let id = self.create_concept_in_memory(braindump_id, label, vec);
        InMemoryResolution::Created { id }
    }

    /// Create a concept, store its embedding (identity + retrieval seed), and
    /// record this braindump as its first extractor (ADR-0010).
    fn create_concept_in_memory(&self, braindump_id: i64, label: &str, vec: &[f32]) -> i64 {
        let id = Self::next_id(&self.next_concept_id);
        let created_at = now_seconds();
        self.concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(
                id,
                Concept {
                    id,
                    label: label.to_string(),
                    created_at,
                },
            );
        self.concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .insert(id, vec.to_vec());
        self.add_concept_provenance(id, braindump_id);
        id
    }

    /// Create an edge and seed its type history at index 0 = the original
    /// assertion (ADR-0003).
    fn create_edge_in_memory(&self, source_id: i64, target_id: i64, original_type: &str) -> i64 {
        let id = Self::next_id(&self.next_edge_id);
        let created_at = now_seconds();
        let edge = Edge {
            id,
            source_concept_id: source_id,
            target_concept_id: target_id,
            original_type: original_type.to_string(),
            created_at,
        };
        self.add_edge(edge);
        id
    }

    /// Insert a merge suggestion row (ADR-0001 borderline pair).
    fn insert_merge_suggestion_in_memory(
        &self,
        braindump_id: i64,
        new_label: &str,
        new_concept_id: i64,
        existing_concept_id: i64,
        similarity: f32,
    ) {
        let id = Self::next_id(&self.next_suggestion_id);
        let created_at = now_seconds();
        self.merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .push(MergeSuggestion {
                id,
                kind: "concept".to_string(),
                braindump_id,
                new_concept_label: new_label.to_string(),
                new_concept_id,
                existing_concept_id,
                similarity,
                status: "pending".to_string(),
                created_at,
            });
    }

    /// Fold `fold_id` into `keep_id` (the survivor), mirroring
    /// [`merge_concepts_conn`] in the Sqlite adapter: repoint/merge edges
    /// touching the fold concept, union extraction provenance, drop the fold
    /// concept's embedding, then delete the fold concept — its remaining
    /// provenance and merge suggestions referencing it are cleaned up
    /// (ADR-0001 / ADR-0010).
    fn merge_concepts_in_memory(&self, keep_id: i64, fold_id: i64) {
        // Edges touching the fold concept: merge duplicates (union provenance)
        // and repoint the rest onto the survivor.
        let fold_edges: Vec<Edge> = self
            .edges
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .values()
            .filter(|e| e.source_concept_id == fold_id || e.target_concept_id == fold_id)
            .cloned()
            .collect();
        for edge in fold_edges {
            let new_src = if edge.source_concept_id == fold_id {
                keep_id
            } else {
                edge.source_concept_id
            };
            let new_tgt = if edge.target_concept_id == fold_id {
                keep_id
            } else {
                edge.target_concept_id
            };
            // Check for collision: same (source, original_type, target) already
            // on the survivor.
            let collision_id = self
                .edges
                .lock()
                .expect("InMemoryGraphRepo mutex poisoned")
                .values()
                .find(|e| {
                    e.id != edge.id
                        && e.source_concept_id == new_src
                        && e.original_type == edge.original_type
                        && e.target_concept_id == new_tgt
                })
                .map(|e| e.id);
            if let Some(keeper_edge_id) = collision_id {
                // Union provenance onto the keeper, drop the fold edge.
                let fold_prov = self
                    .edge_provenance
                    .lock()
                    .expect("InMemoryGraphRepo mutex poisoned")
                    .get(&edge.id)
                    .cloned()
                    .unwrap_or_default();
                for bd in fold_prov {
                    self.add_edge_provenance(keeper_edge_id, bd);
                }
                self.edges
                    .lock()
                    .expect("InMemoryGraphRepo mutex poisoned")
                    .remove(&edge.id);
                self.edge_type_history
                    .lock()
                    .expect("InMemoryGraphRepo mutex poisoned")
                    .remove(&edge.id);
                self.edge_provenance
                    .lock()
                    .expect("InMemoryGraphRepo mutex poisoned")
                    .remove(&edge.id);
            } else {
                // Repoint the edge onto the survivor.
                let mut edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
                if let Some(e) = edges.get_mut(&edge.id) {
                    e.source_concept_id = new_src;
                    e.target_concept_id = new_tgt;
                }
            }
        }

        // Union extraction provenance: the fold concept's extractors accrete
        // onto the survivor (ADR-0010).
        let fold_prov = self
            .concept_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .get(&fold_id)
            .cloned()
            .unwrap_or_default();
        for bd in fold_prov {
            self.add_concept_provenance(keep_id, bd);
        }

        // Clean fold concept's embedding + delete the fold concept + its
        // provenance. Merge suggestions referencing it are removed (the
        // approved one + any others — FK cascade in Sqlite).
        self.concept_embeddings
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .remove(&fold_id);
        self.concepts
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .remove(&fold_id);
        self.concept_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .remove(&fold_id);
        self.merge_suggestions
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .retain(|s| s.new_concept_id != fold_id && s.existing_concept_id != fold_id);
    }

    // --- issue #47: chat write-back + ontology + retrieval helpers ---

    /// Whether `source —[type]→ target` exists wearing `type` as its current
    /// projected type (ADR-0003) — the structural-inference traversability
    /// check, in-memory.
    fn edge_exists_with_current_type_in_memory(
        &self,
        source_id: i64,
        type_slug: &str,
        target_id: i64,
    ) -> bool {
        let edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
        let history = self
            .edge_type_history
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        edges.values().any(|e| {
            e.source_concept_id == source_id
                && e.target_concept_id == target_id
                && history
                    .get(&e.id)
                    .and_then(|entries| {
                        entries
                            .iter()
                            .max_by_key(|h| h.seq_index)
                            .map(|h| h.type_slug == type_slug)
                    })
                    .unwrap_or(false)
        })
    }

    /// Compute retagged edges for the delta-sync read (issue #48): edges
    /// created before `since` that have a type-history entry with
    /// `seq_index > 0` and `created_at > since`. The InMemoryGraphRepo does
    /// not track tombstones, so deletions are always empty — only additions
    /// and retags are computed here.
    async fn compute_retagged_edges_in_memory(&self, since: i64) -> Vec<RetaggedEdge> {
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
        let mut retagged = Vec::new();
        for (edge_id, entries) in &history {
            let Some(edge) = edges.get(edge_id) else {
                continue;
            };
            if edge.created_at > since {
                continue;
            }
            let has_retag = entries
                .iter()
                .any(|h| h.seq_index > 0 && h.created_at > since);
            if !has_retag {
                continue;
            }
            let current_type = entries
                .iter()
                .max_by_key(|h| h.seq_index)
                .map(|h| h.type_slug.clone())
                .unwrap_or_default();
            retagged.push(RetaggedEdge {
                id: edge.id,
                source_concept_id: edge.source_concept_id,
                target_concept_id: edge.target_concept_id,
                original_type: edge.original_type.clone(),
                current_type,
            });
        }
        retagged.sort_by_key(|r| r.id);
        retagged
    }

    /// Compute the braindump ids whose edges formed the thematic density of a
    /// cluster (ADR-0009): distinct braindumps that asserted edges where both
    /// endpoints are in the cluster and the edge is not a self-edge.
    fn compute_cluster_braindump_ids_in_memory(&self, cluster: &[i64]) -> Vec<i64> {
        let cluster_set: std::collections::HashSet<i64> = cluster.iter().copied().collect();
        let edges = self.edges.lock().expect("InMemoryGraphRepo mutex poisoned");
        let edge_prov = self
            .edge_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        let mut ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
        for e in edges.values() {
            if e.source_concept_id == e.target_concept_id {
                continue;
            }
            if cluster_set.contains(&e.source_concept_id)
                && cluster_set.contains(&e.target_concept_id)
            {
                if let Some(prov) = edge_prov.get(&e.id) {
                    for bd in prov {
                        ids.insert(*bd);
                    }
                }
            }
        }
        let mut result: Vec<i64> = ids.into_iter().collect();
        result.sort_unstable();
        result
    }

    /// Transition a chat-inference proposal's status + resolved_at (in-memory).
    fn transition_proposal_status_in_memory(&self, id: i64, new_status: &str) {
        let now = now_seconds();
        let mut proposals = self
            .chat_inference_proposals
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned");
        if let Some(p) = proposals.iter_mut().find(|p| p.id == id) {
            p.status = new_status.to_string();
            p.resolved_at = Some(now);
        }
    }

    /// Collect braindumps from the traversed subgraph (in-memory): each visited
    /// concept's extraction provenance (ADR-0010). Score decays with hop
    /// distance.
    fn collect_subgraph_braindumps_in_memory(
        &self,
        concept_hops: &HashMap<i64, usize>,
    ) -> Vec<RetrievedBraindump> {
        let mut best: HashMap<i64, f32> = HashMap::new();
        let prov = self
            .concept_provenance
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        for (cid, hops) in concept_hops {
            let score = 1.0 / (1.0 + *hops as f32);
            if let Some(bds) = prov.get(cid) {
                for bd_id in bds {
                    let entry = best.entry(*bd_id).or_insert(0.0);
                    if score > *entry {
                        *entry = score;
                    }
                }
            }
        }
        let braindumps = self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let mut result = Vec::new();
        for (bd_id, score) in &best {
            if let Some(b) = braindumps.get(bd_id) {
                result.push(RetrievedBraindump {
                    id: b.id,
                    verbatim: b.verbatim.clone(),
                    cleaned: b.cleaned.clone(),
                    created_at: b.created_at,
                    score: *score,
                    source: BraindumpSource::Subgraph,
                });
            }
        }
        result
    }

    /// No-seed fallback (ADR-0004): braindump-vector-direct (in-memory).
    async fn no_seed_fallback_in_memory(&self, query_vec: &[f32]) -> Result<RetrievalResult> {
        let hits = self.knn_braindumps(query_vec, BRAINDUMP_TOP_K).await?;
        let braindumps = self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let mut result = Vec::new();
        for (bd_id, sim) in &hits {
            if let Some(b) = braindumps.get(bd_id) {
                result.push(RetrievedBraindump {
                    id: b.id,
                    verbatim: b.verbatim.clone(),
                    cleaned: b.cleaned.clone(),
                    created_at: b.created_at,
                    score: *sim,
                    source: BraindumpSource::VectorDirect,
                });
            }
        }
        Ok(RetrievalResult {
            braindumps: result,
            paths: Vec::new(),
            mode: RetrievalMode::NoSeedFallback,
        })
    }

    /// Braindump-embedding KNN backfill for strays the graph missed
    /// (ADR-0004), in-memory.
    async fn backfill_braindumps_in_memory(
        &self,
        query_vec: &[f32],
        subgraph: &[RetrievedBraindump],
    ) -> Vec<RetrievedBraindump> {
        let already: HashSet<i64> = subgraph.iter().map(|b| b.id).collect();
        let hits = self
            .knn_braindumps(query_vec, BRAINDUMP_TOP_K)
            .await
            .unwrap_or_default();
        let braindumps = self
            .braindumps
            .lock()
            .expect("InMemoryGraphRepo mutex poisoned")
            .clone();
        let mut result = Vec::new();
        for (bd_id, sim) in &hits {
            if already.contains(bd_id) {
                continue;
            }
            if let Some(b) = braindumps.get(bd_id) {
                result.push(RetrievedBraindump {
                    id: b.id,
                    verbatim: b.verbatim.clone(),
                    cleaned: b.cleaned.clone(),
                    created_at: b.created_at,
                    score: *sim,
                    source: BraindumpSource::Backfill,
                });
            }
        }
        result
    }
}

/// The in-memory analogue of [`ConceptResolution`] — how the accretion
/// pipeline resolved one extracted concept.
#[cfg(any(test, feature = "test-support"))]
enum InMemoryResolution {
    Accreted(i64),
    Created { id: i64 },
    Suggested { new_id: i64 },
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

    // --- issue #46: write-path trait methods against InMemoryGraphRepo ---

    /// Helper: build an `ExtractionResult` from label + edge slices.
    fn extraction(concepts: &[&str], edges: &[(&str, &str, &str)]) -> ExtractionResult {
        ExtractionResult {
            concepts: concepts
                .iter()
                .map(|l| ExtractedConcept {
                    label: l.to_string(),
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(s, t, tg)| ExtractedEdge {
                    from_label: s.to_string(),
                    type_slug: t.to_string(),
                    to_label: tg.to_string(),
                })
                .collect(),
        }
    }

    /// Ingest an extraction through the write trait method and verify the
    /// resulting read state: concepts created, edge created with type history
    /// at index 0, provenance recorded, and braindump embedding stored —
    /// mirroring the Sqlite-backed `new_concept_created_with_provenance_and_
    /// embedding` + `edge_accretes_provenance_and_inits_type_history_at_index_
    /// zero` tests in `graph.rs`.
    #[tokio::test]
    async fn in_memory_ingest_creates_concepts_edges_provenance_and_embeddings() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        let bd = repo
            .insert_braindump("maria endangers q3", "maria endangers q3")
            .await
            .unwrap();

        let outcome = repo
            .ingest_extraction(
                bd.id,
                vec![0.5, 0.5],
                extraction(
                    &["Maria", "Q3 launch"],
                    &[("Maria", "endangers", "Q3 launch")],
                ),
                vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            )
            .await
            .unwrap();

        assert_eq!(outcome.concepts_created, 2, "{outcome:?}");
        assert_eq!(outcome.edges_created, 1, "{outcome:?}");
        assert_eq!(outcome.concepts_accreted, 0);
        assert_eq!(outcome.edges_accreted, 0);
        assert_eq!(outcome.edges_rejected, 0);

        let maria = repo
            .concept_id_for_label("Maria")
            .await
            .unwrap()
            .expect("Maria created");
        let q3 = repo
            .concept_id_for_label("Q3 launch")
            .await
            .unwrap()
            .expect("Q3 created");
        assert_eq!(
            repo.concept_provenance(maria).await.unwrap(),
            vec![bd.id],
            "extraction provenance (ADR-0010)"
        );
        assert_eq!(repo.concept_provenance(q3).await.unwrap(), vec![bd.id]);

        let edges = repo.all_edges_with_current_type().await.unwrap();
        assert_eq!(edges.len(), 1, "one edge");
        let e = &edges[0];
        assert_eq!(e.source_concept_id, maria);
        assert_eq!(e.target_concept_id, q3);
        assert_eq!(e.original_type, "endangers");
        assert_eq!(
            e.current_type, e.original_type,
            "fresh edge: current == original (projected from history index 0)"
        );

        assert_eq!(
            repo.edge_provenance(e.id).await.unwrap(),
            vec![bd.id],
            "edge asserted_by this braindump (ADR-0002)"
        );
        let history = repo.edge_type_history(e.id).await.unwrap();
        assert_eq!(history.len(), 1, "type history seeded at index 0");
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");

        assert!(
            repo.braindump_embedding_stored(bd.id).await.unwrap(),
            "braindump embedding stored (retrieval backfill)"
        );
    }

    /// Same concept label + vector across two braindumps → cosine 1.0 ≥ 0.95 →
    /// accretes into one node, not two. Both braindumps appear in the concept's
    /// extraction provenance (ADR-0010). Mirrors the Sqlite-backed
    /// `same_concept_accretes_into_one_node_across_two_braindumps` test.
    #[tokio::test]
    async fn in_memory_ingest_accretes_same_concept_across_two_braindumps() {
        let repo = InMemoryGraphRepo::new();
        let bd1 = repo
            .insert_braindump("q3 review one", "q3 review one")
            .await
            .unwrap();
        let bd2 = repo
            .insert_braindump("q3 review two", "q3 review two")
            .await
            .unwrap();

        let ext = extraction(&["Q3 review"], &[]);
        let vecs = vec![vec![1.0, 0.0]];

        repo.ingest_extraction(bd1.id, vec![0.5, 0.5], ext.clone(), vecs.clone())
            .await
            .unwrap();
        let outcome = repo
            .ingest_extraction(bd2.id, vec![0.5, 0.5], ext, vecs)
            .await
            .unwrap();

        assert_eq!(outcome.concepts_created, 0, "{outcome:?}");
        assert_eq!(outcome.concepts_accreted, 1, "{outcome:?}");

        assert_eq!(
            repo.all_concepts().await.unwrap().len(),
            1,
            "one concept node, not two"
        );
        let cid = repo
            .concept_id_for_label("Q3 review")
            .await
            .unwrap()
            .unwrap();
        let mut prov = repo.concept_provenance(cid).await.unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1.id, bd2.id], "both braindumps in provenance");
    }

    /// Unsanctioned edge type → rejected (ADR-0002: the LLM never invents a
    /// type). Mirrors `unsanctioned_edge_type_is_rejected`.
    #[tokio::test]
    async fn in_memory_ingest_rejects_unsanctioned_edge_type() {
        let repo = InMemoryGraphRepo::new();
        let bd = repo
            .insert_braindump("maria bamboozles q3", "maria bamboozles q3")
            .await
            .unwrap();

        let outcome = repo
            .ingest_extraction(
                bd.id,
                vec![0.5, 0.5],
                extraction(
                    &["Maria", "Q3 launch"],
                    &[("Maria", "bamboozles", "Q3 launch")],
                ),
                vec![vec![1.0, 0.0], vec![0.0, 1.0]],
            )
            .await
            .unwrap();

        assert_eq!(outcome.edges_rejected, 1, "{outcome:?}");
        assert_eq!(outcome.edges_created, 0);
        assert_eq!(
            repo.all_edges_with_current_type().await.unwrap().len(),
            0,
            "unsanctioned edge not stored"
        );
    }

    /// A borderline concept match (cosine in [0.80, 0.95)) creates a new
    /// concept AND a merge suggestion (ADR-0001). Mirrors
    /// `borderline_match_creates_concept_and_merge_suggestion`.
    #[tokio::test]
    async fn in_memory_ingest_borderline_match_creates_concept_and_merge_suggestion() {
        let repo = InMemoryGraphRepo::new();
        let bd1 = repo.insert_braindump("alpha", "alpha").await.unwrap();
        let bd2 = repo
            .insert_braindump("alpha variant", "alpha variant")
            .await
            .unwrap();

        // First: create "alpha" with vector [1, 0].
        repo.ingest_extraction(
            bd1.id,
            vec![0.0, 0.0],
            extraction(&["alpha"], &[]),
            vec![vec![1.0, 0.0]],
        )
        .await
        .unwrap();
        let existing = repo.concept_id_for_label("alpha").await.unwrap().unwrap();

        // Second: "alpha variant" at cosine 0.9 to [1, 0] → suggestion band.
        // [0.9, sqrt(1 − 0.81)] is unit-length and cosine 0.9 to [1, 0].
        let variant_vec = vec![0.9, (1.0_f32 - 0.9 * 0.9).sqrt()];
        let outcome = repo
            .ingest_extraction(
                bd2.id,
                vec![0.0, 0.0],
                extraction(&["alpha variant"], &[]),
                vec![variant_vec],
            )
            .await
            .unwrap();

        assert_eq!(outcome.merge_suggestions, 1, "{outcome:?}");
        assert_eq!(outcome.concepts_created, 1);
        assert_eq!(outcome.concepts_accreted, 0);

        let new_id = repo
            .concept_id_for_label("alpha variant")
            .await
            .unwrap()
            .expect("borderline concept created");
        let suggestions = repo.merge_suggestions().await.unwrap();
        assert_eq!(suggestions.len(), 1, "{suggestions:?}");
        let s = &suggestions[0];
        assert_eq!(s.kind, "concept");
        assert_eq!(s.braindump_id, bd2.id);
        assert_eq!(s.new_concept_label, "alpha variant");
        assert_eq!(s.new_concept_id, new_id);
        assert_eq!(s.existing_concept_id, existing);
        assert_eq!(s.status, "pending");
        assert!(
            (s.similarity - 0.9).abs() < 1e-5,
            "similarity is the cosine: {}",
            s.similarity
        );
    }

    /// Delete a braindump → concept vanishes when its last extracting braindump
    /// is removed (ADR-0010). Mirrors `delete_braindump_drops_extraction_
    /// provenance_and_vanishes_on_last_extractor`.
    #[tokio::test]
    async fn in_memory_delete_braindump_vanishes_concept_on_last_extractor() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        let bd1 = repo.insert_braindump("q3 one", "q3 one").await.unwrap();
        let bd2 = repo.insert_braindump("q3 two", "q3 two").await.unwrap();

        repo.ingest_extraction(
            bd1.id,
            vec![0.5, 0.5],
            extraction(&["Q3"], &[]),
            vec![vec![1.0, 0.0]],
        )
        .await
        .unwrap();
        repo.ingest_extraction(
            bd2.id,
            vec![0.5, 0.5],
            extraction(&["Q3"], &[]),
            vec![vec![1.0, 0.0]],
        )
        .await
        .unwrap();
        let cid = repo.concept_id_for_label("Q3").await.unwrap().unwrap();
        let mut prov = repo.concept_provenance(cid).await.unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1.id, bd2.id]);

        // Delete bd1: Q3 still extracted by bd2 → survives, provenance = [bd2].
        assert!(
            repo.delete_braindump(bd1.id).await.unwrap(),
            "deleting an existing braindump reports true"
        );
        assert_eq!(repo.concept_provenance(cid).await.unwrap(), vec![bd2.id]);
        assert!(
            repo.get_concept(cid).await.unwrap().is_some(),
            "concept survives while another braindump extracts it"
        );

        // Delete bd2: Q3's last extractor gone → concept vanishes.
        assert!(repo.delete_braindump(bd2.id).await.unwrap());
        assert!(
            repo.get_concept(cid).await.unwrap().is_none(),
            "concept vanishes when its last extracting braindump is deleted"
        );
    }

    /// Delete a braindump → edge vanishes when its last asserter is removed
    /// (ADR-0002). Mirrors `delete_braindump_drops_edge_provenance_and_
    /// vanishes_on_last_asserter`.
    #[tokio::test]
    async fn in_memory_delete_braindump_vanishes_edge_on_last_asserter() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        let bd1 = repo
            .insert_braindump("maria endangers q3", "maria endangers q3")
            .await
            .unwrap();
        let bd2 = repo
            .insert_braindump("maria still endangers q3", "maria still endangers q3")
            .await
            .unwrap();

        let ext = extraction(
            &["Maria", "Q3 launch"],
            &[("Maria", "endangers", "Q3 launch")],
        );
        let vecs = vec![vec![1.0, 0.0], vec![0.0, 1.0]];
        repo.ingest_extraction(bd1.id, vec![0.5, 0.5], ext.clone(), vecs.clone())
            .await
            .unwrap();
        repo.ingest_extraction(bd2.id, vec![0.5, 0.5], ext, vecs)
            .await
            .unwrap();

        let maria = repo.concept_id_for_label("Maria").await.unwrap().unwrap();
        let q3 = repo
            .concept_id_for_label("Q3 launch")
            .await
            .unwrap()
            .unwrap();
        let edge = repo
            .find_edge(maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");

        // Delete bd1: edge still asserted by bd2 → survives.
        repo.delete_braindump(bd1.id).await.unwrap();
        assert_eq!(repo.edge_provenance(edge.id).await.unwrap(), vec![bd2.id]);
        assert!(
            repo.find_edge(maria, "endangers", q3)
                .await
                .unwrap()
                .is_some(),
            "edge survives while another braindump asserts it"
        );

        // Delete bd2: last asserter gone → edge vanishes.
        repo.delete_braindump(bd2.id).await.unwrap();
        assert!(
            repo.find_edge(maria, "endangers", q3)
                .await
                .unwrap()
                .is_none(),
            "edge vanishes when its last asserter is deleted"
        );
    }

    /// Delete a missing braindump → false (no error).
    #[tokio::test]
    async fn in_memory_delete_missing_braindump_returns_false() {
        let repo = InMemoryGraphRepo::new();
        assert!(
            !repo.delete_braindump(9999).await.unwrap(),
            "deleting a non-existent braindump reports false"
        );
    }

    /// Approve a merge suggestion: union extraction provenance, fold edges onto
    /// the survivor, drop the fold concept and the suggestion. Mirrors
    /// `approve_merge_unions_extraction_provenance_and_drops_fold_concept` +
    /// `approve_merge_folds_edges_onto_surviving_concept`.
    #[tokio::test]
    async fn in_memory_approve_merge_unions_provenance_folds_edges_drops_fold() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        repo.add_ontology_type("helps", "Helps", "A benefits B.");
        let bd1 = repo
            .insert_braindump("maria endangers q3", "maria endangers q3")
            .await
            .unwrap();
        let bd2 = repo
            .insert_braindump("beta helps q3", "beta helps q3")
            .await
            .unwrap();

        // Maria —[endangers]→ Q3, extracted by bd1.
        repo.ingest_extraction(
            bd1.id,
            vec![0.5, 0.5],
            extraction(&["Maria", "Q3"], &[("Maria", "endangers", "Q3")]),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        )
        .await
        .unwrap();
        // Beta —[helps]→ Q3, extracted by bd2. Beta's vector is unit-length at
        // cosine 0.9 to Maria — inside the suggestion band [0.80, 0.95) — so a
        // borderline concept + suggestion is created. Q3's vector is identical
        // to bd1's → accretes.
        let beta_vec = vec![0.9, (1.0_f32 - 0.9 * 0.9).sqrt()];
        repo.ingest_extraction(
            bd2.id,
            vec![0.5, 0.5],
            extraction(&["Beta", "Q3"], &[("Beta", "helps", "Q3")]),
            vec![beta_vec, vec![0.0, 1.0]],
        )
        .await
        .unwrap();

        let maria = repo.concept_id_for_label("Maria").await.unwrap().unwrap();
        let beta = repo.concept_id_for_label("Beta").await.unwrap().unwrap();
        let q3 = repo.concept_id_for_label("Q3").await.unwrap().unwrap();

        // Seed a pending suggestion: fold Beta into Maria (id 100, distinct from
        // the auto-generated suggestion the accretion created).
        repo.add_merge_suggestion(MergeSuggestion {
            id: 100,
            kind: "concept".into(),
            braindump_id: bd2.id,
            new_concept_label: "Beta".into(),
            new_concept_id: beta,
            existing_concept_id: maria,
            similarity: 0.9,
            status: "pending".into(),
            created_at: 0,
        });

        repo.approve_merge_suggestion(100).await.unwrap();

        // Union extraction provenance onto Maria.
        let mut prov = repo.concept_provenance(maria).await.unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1.id, bd2.id], "union provenance (ADR-0010)");
        // Fold concept (Beta) is gone.
        assert!(
            repo.get_concept(beta).await.unwrap().is_none(),
            "fold concept deleted on approve"
        );
        // Beta's edge (Beta→Q3[helps]) folded onto Maria → Maria→Q3[helps].
        let folded = repo
            .find_edge(maria, "helps", q3)
            .await
            .unwrap()
            .expect("folded edge present");
        assert_eq!(repo.edge_provenance(folded.id).await.unwrap(), vec![bd2.id]);
        // Maria's own edge (endangers) still present — contradictory edges coexist.
        assert!(
            repo.find_edge(maria, "endangers", q3)
                .await
                .unwrap()
                .is_some(),
            "pre-existing edge preserved"
        );
        // The suggestion is consumed (both the test-seeded one and the
        // accretion-generated one, which referenced Beta and cascaded away).
        assert!(
            repo.merge_suggestions()
                .await
                .unwrap()
                .iter()
                .all(|s| s.id != 100),
            "approved suggestion dropped from the queue"
        );
    }

    /// Approving a duplicate-edge collision unions provenance (ADR-0002
    /// accretion). Mirrors `approve_merge_unions_provenance_when_duplicate_
    /// edges_collide`.
    #[tokio::test]
    async fn in_memory_approve_merge_unions_provenance_on_duplicate_edge_collision() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("helps", "Helps", "A benefits B.");
        let bd1 = repo
            .insert_braindump("maria helps q3", "maria helps q3")
            .await
            .unwrap();
        let bd2 = repo
            .insert_braindump("beta helps q3", "beta helps q3")
            .await
            .unwrap();

        repo.ingest_extraction(
            bd1.id,
            vec![0.5, 0.5],
            extraction(&["Maria", "Q3"], &[("Maria", "helps", "Q3")]),
            vec![vec![1.0, 0.0], vec![0.0, 1.0]],
        )
        .await
        .unwrap();
        let beta_vec = vec![0.9, (1.0_f32 - 0.9 * 0.9).sqrt()];
        repo.ingest_extraction(
            bd2.id,
            vec![0.5, 0.5],
            extraction(&["Beta", "Q3"], &[("Beta", "helps", "Q3")]),
            vec![beta_vec, vec![0.0, 1.0]],
        )
        .await
        .unwrap();

        let maria = repo.concept_id_for_label("Maria").await.unwrap().unwrap();
        let beta = repo.concept_id_for_label("Beta").await.unwrap().unwrap();
        let q3 = repo.concept_id_for_label("Q3").await.unwrap().unwrap();

        repo.add_merge_suggestion(MergeSuggestion {
            id: 200,
            kind: "concept".into(),
            braindump_id: bd2.id,
            new_concept_label: "Beta".into(),
            new_concept_id: beta,
            existing_concept_id: maria,
            similarity: 0.9,
            status: "pending".into(),
            created_at: 0,
        });

        repo.approve_merge_suggestion(200).await.unwrap();

        // Both asserted →Q3[helps]; after fold they collide on (Maria, helps, Q3)
        // → one edge, provenance unioned.
        let edge = repo
            .find_edge(maria, "helps", q3)
            .await
            .unwrap()
            .expect("merged edge present");
        let mut prov = repo.edge_provenance(edge.id).await.unwrap();
        prov.sort_unstable();
        assert_eq!(
            prov,
            vec![bd1.id, bd2.id],
            "duplicate edges merged, provenance unioned"
        );
        assert_eq!(
            repo.all_edges_with_current_type().await.unwrap().len(),
            1,
            "one edge, not two"
        );
        assert!(repo.get_concept(beta).await.unwrap().is_none());
    }

    /// Reject a merge suggestion: keep both concepts, drop the suggestion.
    /// Mirrors `reject_merge_keeps_concepts_separate_and_drops_suggestion`.
    #[tokio::test]
    async fn in_memory_reject_merge_keeps_concepts_and_drops_suggestion() {
        let repo = InMemoryGraphRepo::new();
        let bd1 = repo.insert_braindump("maria", "maria").await.unwrap();
        let bd2 = repo.insert_braindump("beta", "beta").await.unwrap();

        repo.ingest_extraction(
            bd1.id,
            vec![0.5, 0.5],
            extraction(&["Maria"], &[]),
            vec![vec![1.0, 0.0]],
        )
        .await
        .unwrap();
        repo.ingest_extraction(
            bd2.id,
            vec![0.5, 0.5],
            extraction(&["Beta"], &[]),
            vec![vec![0.0, 1.0]],
        )
        .await
        .unwrap();

        let maria = repo.concept_id_for_label("Maria").await.unwrap().unwrap();
        let beta = repo.concept_id_for_label("Beta").await.unwrap().unwrap();

        repo.add_merge_suggestion(MergeSuggestion {
            id: 300,
            kind: "concept".into(),
            braindump_id: bd2.id,
            new_concept_label: "Beta".into(),
            new_concept_id: beta,
            existing_concept_id: maria,
            similarity: 0.9,
            status: "pending".into(),
            created_at: 0,
        });

        repo.reject_merge_suggestion(300).await.unwrap();

        assert!(
            repo.get_concept(maria).await.unwrap().is_some(),
            "keeper survives"
        );
        assert!(
            repo.get_concept(beta).await.unwrap().is_some(),
            "fold concept survives reject"
        );
        assert_eq!(
            repo.concept_provenance(maria).await.unwrap(),
            vec![bd1.id],
            "provenance unchanged on reject"
        );
        assert!(
            repo.merge_suggestions()
                .await
                .unwrap()
                .iter()
                .all(|s| s.id != 300),
            "rejected suggestion dropped from the queue"
        );
    }

    /// Approving / rejecting a missing suggestion → NotFound. Mirrors
    /// `approve_missing_suggestion_is_not_found` + `reject_missing_suggestion_
    /// is_not_found`.
    #[tokio::test]
    async fn in_memory_approve_and_reject_missing_suggestion_is_not_found() {
        let repo = InMemoryGraphRepo::new();
        let approve = repo.approve_merge_suggestion(9999).await;
        assert!(
            matches!(approve, Err(Error::NotFound(_))),
            "approve missing: {approve:?}"
        );
        let reject = repo.reject_merge_suggestion(9999).await;
        assert!(
            matches!(reject, Err(Error::NotFound(_))),
            "reject missing: {reject:?}"
        );
    }

    // --- issue #47: chat write-back (ADR-0006) against InMemoryGraphRepo ---

    /// Helper: build an `EvidenceEdge` hop for the structural-inference path.
    fn hop(source: i64, edge_type: &str, target: i64) -> EvidenceEdge {
        EvidenceEdge {
            source_concept_id: source,
            edge_type: edge_type.to_string(),
            target_concept_id: target,
        }
    }

    /// ADR-0006: a structural proposal enters the queue pending. No
    /// auto-endorse — no edge is persisted. Mirrors `propose_with_traversable_
    /// path_creates_pending_proposal_and_no_edge`.
    #[tokio::test]
    async fn in_memory_propose_structural_stores_pending_proposal_and_no_edge() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        // Seed edge 1 —[causes]→ 2 so the single-hop evidence path is traversable.
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });

        let proposal = repo
            .propose_structural_inference(
                1,
                2,
                "causes",
                vec![hop(1, "causes", 2)],
                Some("summary"),
            )
            .await
            .unwrap();

        assert_eq!(proposal.mode, STRUCTURAL_MODE);
        assert_eq!(proposal.status, STATUS_PENDING);
        assert_eq!(proposal.source_concept_id, 1);
        assert_eq!(proposal.target_concept_id, 2);
        assert_eq!(proposal.proposed_type, "causes");
        assert_eq!(proposal.evidence_path, vec![hop(1, "causes", 2)]);
        assert_eq!(proposal.rationale.as_deref(), Some("summary"));
        assert!(
            proposal.snapshot.is_none(),
            "structural carries no snapshot"
        );
        assert!(proposal.resolved_at.is_none());

        assert_eq!(
            repo.list_inference_proposals().await.unwrap().len(),
            1,
            "exactly one proposal queued"
        );
        // No new edge persisted — the only edge is the seed 1 —[causes]→ 2.
        assert_eq!(
            repo.all_edges_with_current_type().await.unwrap().len(),
            1,
            "no edge persisted on a pending proposal (no auto-endorse)"
        );
        assert!(
            repo.edge_inference_asserted_by(10)
                .await
                .unwrap()
                .is_empty(),
            "no inference provenance written on a pending proposal"
        );
    }

    /// ADR-0002: the LLM never invents a type. An unsanctioned proposed type
    /// is rejected and the caller is directed to the ontology governance queue.
    /// Mirrors `propose_with_unsanctioned_type_is_rejected_and_directed_to_
    /// ontology_queue`.
    #[tokio::test]
    async fn in_memory_propose_structural_rejects_unsanctioned_type() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });

        let err = repo
            .propose_structural_inference(1, 2, "bogus", vec![hop(1, "causes", 2)], None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(
            err.to_string().contains("/ontology/propose"),
            "directed to the ontology queue: {err:?}"
        );
        assert!(
            repo.list_inference_proposals().await.unwrap().is_empty(),
            "no proposal created for an unsanctioned type"
        );
    }

    /// ADR-0006 thematic mode + ADR-0009: a thematic proposal carries a frozen
    /// Thematic Snapshot — the braindump ids whose edges formed the cluster's
    /// density, plus the cluster's concept composition. Pending — no auto-
    /// endorse. Mirrors `propose_thematic_inference_creates_pending_proposal_
    /// with_frozen_snapshot`.
    #[tokio::test]
    async fn in_memory_propose_thematic_stores_pending_proposal_with_snapshot() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        repo.add_ontology_type("depends_on", "Depends on", "A needs B.");
        for (id, label) in [(1_i64, "Maria"), (2, "Q3 launch"), (3, "Beta release")] {
            repo.add_concept(Concept {
                id,
                label: label.into(),
                created_at: 0,
            });
        }
        // One braindump asserts both cluster edges → it is the snapshot evidence.
        let bd = repo
            .insert_braindump(
                "maria endangers q3 which beta depends on",
                "maria endangers q3 which beta depends on",
            )
            .await
            .unwrap();
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "endangers".into(),
            created_at: 0,
        });
        repo.add_edge_provenance(10, bd.id);
        repo.add_edge(Edge {
            id: 11,
            source_concept_id: 2,
            target_concept_id: 3,
            original_type: "depends_on".into(),
            created_at: 0,
        });
        repo.add_edge_provenance(11, bd.id);

        let proposal = repo
            .propose_thematic_inference(1, 3, "endangers", vec![1, 2, 3], Some("bridge"))
            .await
            .unwrap();

        assert_eq!(proposal.mode, THEMATIC_MODE);
        assert_eq!(proposal.status, STATUS_PENDING);
        assert_eq!(proposal.source_concept_id, 1);
        assert_eq!(proposal.target_concept_id, 3);
        assert_eq!(proposal.proposed_type, "endangers");
        assert!(
            proposal.evidence_path.is_empty(),
            "thematic mode has no evidence path — not graph-backed"
        );
        assert_eq!(proposal.rationale.as_deref(), Some("bridge"));
        assert!(proposal.resolved_at.is_none());

        let snapshot = proposal
            .snapshot
            .as_ref()
            .expect("thematic proposal carries a Thematic Snapshot");
        assert_eq!(
            snapshot.concept_ids,
            vec![1, 2, 3],
            "snapshot captured the cluster's composition"
        );
        assert_eq!(
            snapshot.braindump_ids,
            vec![bd.id],
            "snapshot captured the cluster's braindump evidence"
        );
        // No edge persisted yet — no auto-endorse.
        assert!(
            repo.find_edge(1, "endangers", 3).await.unwrap().is_none(),
            "no edge persisted on a pending thematic proposal"
        );
    }

    /// ADR-0006: on endorsement the edge persists with inference provenance
    /// `asserted_by: [Chat_Inference_ID, mode: structural]`. When the seed edge
    /// already matches (source, original_type, target) the inference accretes
    /// onto it rather than duplicating. Mirrors `endorse_persists_edge_with_
    /// structural_inference_provenance_and_type_history`.
    #[tokio::test]
    async fn in_memory_endorse_persists_edge_with_inference_provenance_and_type_history() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let proposal = repo
            .propose_structural_inference(
                1,
                2,
                "causes",
                vec![hop(1, "causes", 2)],
                Some("summary"),
            )
            .await
            .unwrap();

        let endorsed = repo.endorse_inference_proposal(proposal.id).await.unwrap();
        assert_eq!(endorsed.status, STATUS_ENDORSED);
        assert!(endorsed.resolved_at.is_some());

        // The seed edge accretes the inference provenance — no duplicate edge.
        assert_eq!(
            repo.all_edges_with_current_type().await.unwrap().len(),
            1,
            "edge accreted, not duplicated"
        );
        let edge = repo
            .find_edge(1, "causes", 2)
            .await
            .unwrap()
            .expect("endorsed edge present");
        assert_eq!(edge.id, 10);
        // Type history retains its index-0 seed (ADR-0003).
        let history = repo.edge_type_history(10).await.unwrap();
        assert_eq!(history.len(), 1, "type history seeded at index 0");
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "causes");
        // Provenance: this proposal is the asserter, origin structural, no snapshot.
        let assertions = repo.edge_inference_asserted_by(10).await.unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
        assert!(
            assertions[0].snapshot_id.is_none(),
            "structural provenance has no snapshot"
        );
    }

    /// ADR-0002 accretion: if the direct edge already exists (asserted by a
    /// braindump), endorsing adds the inference as a co-asserter rather than
    /// duplicating the edge. Mirrors `endorse_accretes_provenance_when_direct_
    /// edge_already_exists`.
    #[tokio::test]
    async fn in_memory_endorse_accretes_provenance_when_direct_edge_already_exists() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("endangers", "Endangers", "A threatens B.");
        repo.add_ontology_type("depends_on", "Depends on", "A needs B.");
        for (id, label) in [(1_i64, "Maria"), (2, "Q3 launch"), (3, "Beta release")] {
            repo.add_concept(Concept {
                id,
                label: label.into(),
                created_at: 0,
            });
        }
        let bd_path = repo
            .insert_braindump(
                "maria endangers q3 which beta depends on",
                "maria endangers q3 which beta depends on",
            )
            .await
            .unwrap();
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "endangers".into(),
            created_at: 0,
        });
        repo.add_edge_provenance(10, bd_path.id);
        repo.add_edge(Edge {
            id: 11,
            source_concept_id: 2,
            target_concept_id: 3,
            original_type: "depends_on".into(),
            created_at: 0,
        });
        repo.add_edge_provenance(11, bd_path.id);
        // Separately assert the direct edge Maria —[endangers]→ Beta with a
        // second braindump.
        let bd_direct = repo
            .insert_braindump(
                "maria endangers the beta release directly",
                "maria endangers the beta release directly",
            )
            .await
            .unwrap();
        repo.add_edge(Edge {
            id: 12,
            source_concept_id: 1,
            target_concept_id: 3,
            original_type: "endangers".into(),
            created_at: 0,
        });
        repo.add_edge_provenance(12, bd_direct.id);

        let proposal = repo
            .propose_structural_inference(
                1,
                3,
                "endangers",
                vec![hop(1, "endangers", 2), hop(2, "depends_on", 3)],
                None,
            )
            .await
            .unwrap();
        repo.endorse_inference_proposal(proposal.id).await.unwrap();

        // Same edge (no duplicate), braindump provenance preserved.
        let edge = repo
            .find_edge(1, "endangers", 3)
            .await
            .unwrap()
            .expect("edge still present");
        assert_eq!(edge.id, 12, "edge accreted, not duplicated");
        assert_eq!(
            repo.edge_provenance(edge.id).await.unwrap(),
            vec![bd_direct.id],
            "braindump provenance preserved"
        );
        let assertions = repo.edge_inference_asserted_by(edge.id).await.unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
    }

    /// ADR-0006: a rejected inference never enters the graph. The proposal
    /// stays in the table (audit trail) but is no longer pending. Mirrors
    /// `reject_drops_the_proposal_and_persists_no_edge`.
    #[tokio::test]
    async fn in_memory_reject_drops_proposal_and_persists_no_edge() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let proposal = repo
            .propose_structural_inference(1, 2, "causes", vec![hop(1, "causes", 2)], None)
            .await
            .unwrap();

        let rejected = repo.reject_inference_proposal(proposal.id).await.unwrap();
        assert_eq!(rejected.status, STATUS_REJECTED);
        assert!(rejected.resolved_at.is_some());

        // No new edge — only the seed remains.
        assert_eq!(
            repo.all_edges_with_current_type().await.unwrap().len(),
            1,
            "no edge persisted on reject"
        );
        assert!(
            repo.edge_inference_asserted_by(10)
                .await
                .unwrap()
                .is_empty(),
            "no inference provenance written on reject"
        );
        // The rejected proposal stays in the table but is no longer pending.
        let refreshed = repo
            .get_inference_proposal(proposal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.status, STATUS_REJECTED);
    }

    /// Endorsement is immutable: endorsing a missing proposal is `NotFound`,
    /// and endorsing an already-endorsed proposal is `Conflict`. Mirrors
    /// `endorse_missing_proposal_is_not_found` + `endorse_already_endorsed_
    /// is_conflict`.
    #[tokio::test]
    async fn in_memory_endorse_missing_is_not_found_and_already_endorsed_is_conflict() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let missing = repo.endorse_inference_proposal(9999).await;
        assert!(
            matches!(missing, Err(Error::NotFound(_))),
            "endorse missing: {missing:?}"
        );

        let proposal = repo
            .propose_structural_inference(1, 2, "causes", vec![hop(1, "causes", 2)], None)
            .await
            .unwrap();
        repo.endorse_inference_proposal(proposal.id).await.unwrap();
        let again = repo.endorse_inference_proposal(proposal.id).await;
        assert!(
            matches!(again, Err(Error::Conflict(_))),
            "endorse already-endorsed: {again:?}"
        );
    }

    // --- issue #47: ontology governance + refactor (ADR-0003) ---

    /// `insert_type_proposal` stores the row; `list_type_proposals` returns it
    /// oldest-first and `get_type_proposal` looks it up by id. Mirrors the
    /// governance storage flow in `ontology.rs::tests`.
    #[tokio::test]
    async fn in_memory_insert_type_proposal_then_list_and_get() {
        let repo = InMemoryGraphRepo::new();
        let proposal = repo
            .insert_type_proposal(
                "nurtures".into(),
                "Nurtures".into(),
                "A nurtures B.".into(),
                None,
                "pending".into(),
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(proposal.slug, "nurtures");
        assert_eq!(proposal.label, "Nurtures");
        assert_eq!(proposal.description, "A nurtures B.");
        assert!(proposal.merge_of.is_none());
        assert_eq!(proposal.status, "pending");
        assert!(proposal.near_match_slug.is_none());
        assert!(proposal.near_match_similarity.is_none());
        assert!(proposal.resolved_at.is_none());

        let listed = repo.list_type_proposals().await.unwrap();
        assert_eq!(listed.len(), 1, "exactly one proposal queued");
        assert_eq!(listed[0].id, proposal.id, "oldest-first");
        let got = repo
            .get_type_proposal(proposal.id)
            .await
            .unwrap()
            .expect("proposal found by id");
        assert_eq!(got, proposal);
    }

    /// ADR-0003: approving a `merge_of` proposal adds the new type to the
    /// ontology, then `run_refactor` re-classifies each edge wearing the merged
    /// type and appends the LLM's chosen slug to its type history. With FakeLlm
    /// (which echoes the prompt — not a valid slug) the refactor falls back to
    /// the new slug. Mirrors `run_refactor_with_merge_of_and_no_edges_is_noop`
    /// (the non-noop case) + the `approve_*` governance tests.
    #[tokio::test]
    async fn in_memory_approve_type_proposal_retags_edges_via_refactor() {
        use crate::llm::FakeLlm;

        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let proposal = repo
            .insert_type_proposal(
                "causes_v2".into(),
                "Causes v2".into(),
                "A brings about B, revised.".into(),
                Some("causes".into()),
                "pending".into(),
                None,
                None,
            )
            .await
            .unwrap();
        // The InMemory approve does not run the refactor (the wrapper would);
        // it only adds the type + embedding + flips status. Run the refactor
        // explicitly against the approved proposal.
        repo.approve_type_proposal(
            proposal.id,
            "causes_v2".into(),
            "Causes v2".into(),
            "A brings about B, revised.".into(),
            vec![1.0; 64],
        )
        .await
        .unwrap();
        let approved = repo
            .get_type_proposal(proposal.id)
            .await
            .unwrap()
            .expect("approved proposal present");
        assert_eq!(approved.status, "approved", "approve flipped the status");
        assert!(approved.resolved_at.is_some());

        let outcome = repo
            .run_refactor(&FakeLlm::default(), &approved)
            .await
            .unwrap();
        assert_eq!(
            outcome.edges_retagged, 1,
            "the one `causes` edge was retagged"
        );

        // The edge's current type is now the new slug (history appended).
        assert_eq!(
            repo.current_edge_type(10).await.unwrap().as_deref(),
            Some("causes_v2"),
            "current type projected from the retag"
        );
        assert_eq!(
            repo.edges_with_current_type("causes").await.unwrap(),
            Vec::<i64>::new(),
            "the edge no longer wears `causes`"
        );
        assert_eq!(
            repo.edges_with_current_type("causes_v2").await.unwrap(),
            vec![10],
            "the edge now wears `causes_v2`"
        );
        let history = repo.edge_type_history(10).await.unwrap();
        assert_eq!(history.len(), 2, "original + retag");
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "causes");
        assert_eq!(history[1].seq_index, 1);
        assert_eq!(history[1].type_slug, "causes_v2");
    }

    /// ADR-0003: rejecting a proposal marks it rejected, no ontology change,
    /// no retag — edges keep wearing their current type. Missing id is
    /// `NotFound`. Mirrors `reject_marks_pending_proposal_rejected` +
    /// `reject_missing_proposal_is_not_found`.
    #[tokio::test]
    async fn in_memory_reject_type_proposal_keeps_edges_untouched() {
        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let proposal = repo
            .insert_type_proposal(
                "causes_v2".into(),
                "Causes v2".into(),
                "revised".into(),
                Some("causes".into()),
                "pending".into(),
                None,
                None,
            )
            .await
            .unwrap();
        repo.reject_type_proposal(proposal.id).await.unwrap();
        let rejected = repo
            .get_type_proposal(proposal.id)
            .await
            .unwrap()
            .expect("rejected proposal present");
        assert_eq!(rejected.status, "rejected");
        assert!(rejected.resolved_at.is_some());

        // No retag — the edge still wears `causes`.
        assert_eq!(
            repo.current_edge_type(10).await.unwrap().as_deref(),
            Some("causes"),
            "edges untouched on reject"
        );
        assert_eq!(
            repo.edges_with_current_type("causes").await.unwrap(),
            vec![10],
            "no edges retagged"
        );
        assert!(
            repo.ontology_slugs().await.unwrap() == vec!["causes".to_string()],
            "no ontology change on reject"
        );

        // Missing id → NotFound.
        let missing = repo.reject_type_proposal(9999).await;
        assert!(
            matches!(missing, Err(Error::NotFound(_))),
            "reject missing: {missing:?}"
        );
    }

    /// THE #47 acceptance test: `RefactorRunner` runs the refactor against
    /// `InMemoryGraphRepo` — no real SQLite, no real LLM. Proves the runner's
    /// `spawn(repo: Arc<dyn GraphRepo>, llm: Arc<dyn Llm>, proposal)` +
    /// `await_all` seam works against any adapter. Mirrors the shape of
    /// `run_refactor_with_merge_of_and_no_edges_is_noop` but with edges +
    /// `InMemoryGraphRepo`.
    #[tokio::test]
    async fn refactor_runner_runs_against_in_memory_graph_repo_without_sqlite() {
        use crate::llm::FakeLlm;
        use crate::ontology::RefactorRunner;
        use std::sync::Arc;

        let repo = InMemoryGraphRepo::new();
        repo.add_ontology_type("causes", "Causes", "A brings about B.");
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
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "causes".into(),
            created_at: 0,
        });
        let repo: Arc<dyn GraphRepo> = Arc::new(repo);

        let proposal = repo
            .insert_type_proposal(
                "causes_v2".into(),
                "Causes v2".into(),
                "A brings about B, revised.".into(),
                Some("causes".into()),
                "pending".into(),
                None,
                None,
            )
            .await
            .unwrap();
        repo.approve_type_proposal(
            proposal.id,
            "causes_v2".into(),
            "Causes v2".into(),
            "A brings about B, revised.".into(),
            vec![1.0; 64],
        )
        .await
        .unwrap();
        let approved = repo
            .get_type_proposal(proposal.id)
            .await
            .unwrap()
            .expect("approved proposal present");

        let runner = RefactorRunner::new();
        runner.spawn(
            Arc::clone(&repo),
            Arc::new(FakeLlm::default()) as Arc<dyn Llm>,
            approved,
        );
        runner.await_all().await;

        // The refactor appended the new slug to the edge's type history;
        // FakeLlm echoes the prompt (not a valid slug) so the runner fell back
        // to the proposal's new slug `causes_v2`.
        assert_eq!(
            repo.current_edge_type(10).await.unwrap().as_deref(),
            Some("causes_v2"),
            "RefactorRunner retagged the edge against InMemoryGraphRepo"
        );
        let history = repo.edge_type_history(10).await.unwrap();
        assert_eq!(history.len(), 2, "original + retag");
        assert_eq!(history[0].type_slug, "causes");
        assert_eq!(history[1].type_slug, "causes_v2");
    }

    // --- issue #47: retrieval pipeline (ADR-0004) against InMemoryGraphRepo ---

    /// ADR-0004 seed-then-expand: the query seeds an entry concept by
    /// embedding KNN, the graph traverses typed edges to expand the
    /// neighbourhood, and braindumps collected from the subgraph (via concept
    /// extraction provenance) form the context. Mirrors `expand_finds_
    /// braindump_connected_by_edge_but_not_containing_query_word`.
    #[tokio::test]
    async fn in_memory_retrieve_finds_braindump_via_edge_expansion() {
        let repo = InMemoryGraphRepo::new();
        repo.add_concept(Concept {
            id: 1,
            label: "Maria".into(),
            created_at: 0,
        });
        repo.add_concept(Concept {
            id: 2,
            label: "Q3 launch".into(),
            created_at: 0,
        });
        repo.set_concept_embedding(1, vec![1.0, 0.0]);
        repo.add_edge(Edge {
            id: 10,
            source_concept_id: 1,
            target_concept_id: 2,
            original_type: "endangers".into(),
            created_at: 0,
        });
        // A braindump that extracted Q3 (the expansion target), not the seed.
        // It does not lexically contain the query vector's "concept" — the
        // graph link is what surfaces it.
        let bd = repo
            .insert_braindump(
                "maria leaving tanks the timeline",
                "maria leaving tanks the timeline",
            )
            .await
            .unwrap();
        repo.add_concept_provenance(2, bd.id);

        let result = repo.retrieve(&[1.0, 0.0]).await.unwrap();

        assert_eq!(result.mode, RetrievalMode::SeedThenExpand);
        let found = result
            .braindumps
            .iter()
            .find(|b| b.id == bd.id)
            .expect("the graph-linked braindump is found via expansion");
        assert_eq!(found.source, BraindumpSource::Subgraph);
        assert!(
            result.paths.iter().any(|e| {
                e.source_concept_label == "Maria"
                    && e.target_concept_label == "Q3 launch"
                    && e.edge_type == "endangers"
            }),
            "traversed edge path present: {:?}",
            result.paths
        );
    }

    /// ADR-0004 backfill: a braindump not connected to the seeded subgraph but
    /// whose braindump-embedding is near the query is returned as `Backfill`.
    /// Mirrors `backfill_finds_strays_the_graph_missed`.
    #[tokio::test]
    async fn in_memory_retrieve_backfills_strays_the_graph_missed() {
        let repo = InMemoryGraphRepo::new();
        repo.add_concept(Concept {
            id: 1,
            label: "Maria".into(),
            created_at: 0,
        });
        repo.set_concept_embedding(1, vec![1.0, 0.0]);
        // A graph-linked braindump (in the subgraph via concept provenance).
        let bd_graph = repo
            .insert_braindump(
                "maria endangers the q3 launch",
                "maria endangers the q3 launch",
            )
            .await
            .unwrap();
        repo.add_concept_provenance(1, bd_graph.id);
        // A stray braindump with no concept link, but a near-query embedding.
        let bd_stray = repo
            .insert_braindump("q3 risk assessment notes", "q3 risk assessment notes")
            .await
            .unwrap();
        repo.set_braindump_embedding(bd_stray.id, vec![1.0, 0.0]);

        let result = repo.retrieve(&[1.0, 0.0]).await.unwrap();

        let stray = result
            .braindumps
            .iter()
            .find(|b| b.id == bd_stray.id)
            .expect("stray braindump found via backfill");
        assert_eq!(stray.source, BraindumpSource::Backfill);
        assert!(stray.score > 0.0);
    }

    /// ADR-0004 no-seed fallback: an empty graph has no concept seeds, so
    /// retrieval falls back to braindump-vector-direct — which is also empty.
    /// `mode` is `NoSeedFallback` and no paths are traversed.
    #[tokio::test]
    async fn in_memory_retrieve_returns_empty_on_empty_graph() {
        use crate::llm::FakeLlm;

        let repo = InMemoryGraphRepo::new();
        let result = repo
            .retrieve(&vec![0.0; FakeLlm::default().dim()])
            .await
            .unwrap();

        assert_eq!(result.mode, RetrievalMode::NoSeedFallback);
        assert!(
            result.braindumps.is_empty(),
            "no braindumps on an empty graph"
        );
        assert!(result.paths.is_empty(), "no graph traversal in fallback");
    }

    // --- issue #48: helper-function tests (moved from graph.rs) ---

    #[test]
    fn current_type_subquery_returns_the_projection_fragment() {
        assert_eq!(
            current_type_subquery(),
            "SELECT type_slug FROM edge_type_history WHERE edge_id = e.id ORDER BY seq_index DESC LIMIT 1"
        );
    }

    #[test]
    fn vec_to_blob_encodes_f32_slice_as_little_endian_bytes() {
        let vec = vec![1.0_f32, 0.0, -0.5];
        let blob = vec_to_blob(&vec);
        assert_eq!(blob.len(), 12, "4 bytes per f32");
        assert_eq!(&blob[0..4], &1.0_f32.to_le_bytes());
        assert_eq!(&blob[4..8], &0.0_f32.to_le_bytes());
        assert_eq!(&blob[8..12], &(-0.5_f32).to_le_bytes());
    }
}
