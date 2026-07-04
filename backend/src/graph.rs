//! The atomic accretion pipeline (issue #6, ADR-0001 / ADR-0002 / ADR-0003 /
//! ADR-0010).
//!
//! Given a braindump and the LLM's [`ExtractionResult`] (concepts + edges),
//! this module:
//!  1. embeds the braindump and each extracted concept label via the
//!     [`EmbeddingClient`] seam (Gemini — supersedses the Cohere choice in
//!     `first_draft.md` §C),
//!  2. runs retract → embed-store → identity-resolution → concept accretion →
//!     edge accretion → type-history init → provenance, all inside **one
//!     SQLite transaction** against the in-process `sqlite-vec`.
//!
//! The embedding *computation* (a network call) cannot live inside a sync
//! SQLite transaction, so it runs first; the embedding *storage* and every
//! graph mutation commit atomically together (ADR-0001: a separate vector
//! server would break this).
//!
//! Concept identity is embedding-match (ADR-0001): >95% cosine accretes,
//! borderline surfaces a merge suggestion, below the floor a new concept is
//! born. Edge identity anchors on (source, original_type, target) — the
//! original type is immutable; the *current* type is a projection off the
//! append-only `edge_type_history` (ADR-0003). Provenance is origin-typed in a
//! later slice; here every assertion is braindump-origin.

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::{now_seconds, Db};
use crate::embedding::EmbeddingClient;
use crate::error::Result;
use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};

/// Cosine similarity at or above which a newly-extracted concept is deemed the
/// same as an existing one and accretes silently (ADR-0001: >95%).
pub const ACCRETION_SIMILARITY: f32 = 0.95;

/// Below [`ACCRETION_SIMILARITY`] but at or above this floor, a concept match is
/// ambiguous: a new concept is created and a merge suggestion is surfaced for
/// human confirm/reject (ADR-0001). Below the floor the match is rejected as
/// "different" and a new concept is created with no suggestion.
pub const SUGGESTION_FLOOR_SIMILARITY: f32 = 0.80;

/// What the accretion pipeline did with one braindump's extraction. Returned so
/// the ingest route can log it and tests can assert without raw SQL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct IngestOutcome {
    pub concepts_created: usize,
    pub concepts_accreted: usize,
    pub merge_suggestions: usize,
    pub edges_created: usize,
    pub edges_accreted: usize,
    pub edges_rejected: usize,
}

/// A concept node (read model). Identity is by embedding match, not label
/// equality (ADR-0001); `label` is the LLM's surface form from the first
/// extraction that created it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Concept {
    pub id: i64,
    pub label: String,
    pub created_at: i64,
}

/// A typed, directional, accreting edge (ADR-0002 / ADR-0003). `original_type`
/// anchors identity and is immutable; the current type is the last entry of
/// the edge's type history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    pub id: i64,
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub original_type: String,
    pub created_at: i64,
}

/// One entry in an edge's append-only type history (ADR-0003). Index 0 is the
/// LLM's original assertion; each refactor appends.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeHistoryEntry {
    pub seq_index: i64,
    pub type_slug: String,
    pub created_at: i64,
}

/// A borderline identity pair surfaced for human confirm/reject (ADR-0001).
/// `new_concept_id` is the freshly-created concept; `existing_concept_id` is
/// the near-match it might be the same as. The queue/approval UI is a later
/// slice.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MergeSuggestion {
    pub id: i64,
    pub kind: String,
    pub braindump_id: i64,
    pub new_concept_label: String,
    pub new_concept_id: i64,
    pub existing_concept_id: i64,
    pub similarity: f32,
    pub status: String,
    pub created_at: i64,
}

/// The accretion pipeline entry point. Embeds the braindump and each concept
/// label (Gemini), then commits embedding storage + identity resolution +
/// accretion + provenance + type-history init in one SQLite transaction.
///
/// Idempotent over a braindump: any prior extraction for `braindump_id` is
/// retracted first (concepts/edges losing their last asserter vanish), so this
/// is safe to call on both submit (retracts nothing) and edit (retracts the
/// stale extraction before re-accreting — ADR-0007).
pub async fn ingest_extraction(
    db: &Db,
    embedding: &(dyn EmbeddingClient + Sync),
    braindump_id: i64,
    verbatim: &str,
    extraction: ExtractionResult,
) -> Result<IngestOutcome> {
    let braindump_vec = embedding.embed_document(verbatim).await?;
    let mut concept_vecs = Vec::with_capacity(extraction.concepts.len());
    for concept in &extraction.concepts {
        concept_vecs.push(embedding.embed_document(&concept.label).await?);
    }
    let dim = embedding.dim();
    let concepts = extraction.concepts;
    let edges = extraction.edges;
    db.run(move |conn| {
        // The vec0 tables are dim-dependent (the embedding model fixes the
        // dimensionality), so they are created here rather than in the
        // dim-agnostic schema migration. A cheap `IF NOT EXISTS` metadata check.
        crate::db::ensure_vec_tables(conn, dim)?;
        // Manual transaction control: `Db::run` hands out `&Connection`
        // (immutable), so the `Transaction` API (`&mut self`) is unavailable.
        // `execute_batch` takes `&self`, so BEGIN/COMMIT/ROLLBACK work. The
        // whole extraction + embedding + identity-resolution commit atomically
        // (ADR-0001); any error rolls back — no partial graph state.
        conn.execute_batch("BEGIN")?;
        match accrete(
            conn,
            braindump_id,
            braindump_vec,
            concepts,
            concept_vecs,
            edges,
        ) {
            Ok(outcome) => {
                conn.execute_batch("COMMIT")?;
                Ok(outcome)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    })
    .await
}

/// Run the full accretion for one braindump inside an open transaction.
fn accrete(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    braindump_vec: Vec<f32>,
    concepts: Vec<ExtractedConcept>,
    concept_vecs: Vec<Vec<f32>>,
    edges: Vec<ExtractedEdge>,
) -> Result<IngestOutcome> {
    let mut outcome = IngestOutcome::default();

    retract_extraction(conn, braindump_id)?;

    store_braindump_embedding(conn, braindump_id, &braindump_vec)?;

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
        let resolved = resolve_concept(conn, braindump_id, &concept.label, vec)?;
        match resolved {
            ConceptResolution::Accreted(existing_id) => {
                outcome.concepts_accreted += 1;
                label_to_id.insert(concept.label.clone(), existing_id);
            }
            ConceptResolution::Created { id, .. } => {
                outcome.concepts_created += 1;
                label_to_id.insert(concept.label.clone(), id);
            }
            ConceptResolution::Suggested { new_id, .. } => {
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
        if let Some(edge_id) = find_edge_id(conn, source_id, &edge.type_slug, target_id)? {
            insert_edge_provenance(conn, edge_id, braindump_id)?;
            outcome.edges_accreted += 1;
        } else {
            let edge_id = insert_edge(conn, source_id, target_id, &edge.type_slug)?;
            init_type_history(conn, edge_id, &edge.type_slug)?;
            insert_edge_provenance(conn, edge_id, braindump_id)?;
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
fn resolve_concept(
    conn: &rusqlite::Connection,
    braindump_id: i64,
    label: &str,
    vec: &[f32],
) -> Result<ConceptResolution> {
    if let Some((existing_id, similarity)) = knn_concept(conn, vec)? {
        if similarity >= ACCRETION_SIMILARITY {
            insert_concept_provenance(conn, existing_id, braindump_id)?;
            return Ok(ConceptResolution::Accreted(existing_id));
        }
        if similarity >= SUGGESTION_FLOOR_SIMILARITY {
            let new_id = create_concept(conn, braindump_id, label, vec)?;
            insert_merge_suggestion(conn, braindump_id, label, new_id, existing_id, similarity)?;
            return Ok(ConceptResolution::Suggested { new_id });
        }
    }
    let id = create_concept(conn, braindump_id, label, vec)?;
    Ok(ConceptResolution::Created { id })
}

/// Create a concept, store its embedding (identity + retrieval seed), and record
/// this braindump as its first extractor (ADR-0010).
fn create_concept(
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
    insert_concept_provenance(conn, id, braindump_id)?;
    Ok(id)
}

/// sqlite-vec KNN: nearest concept by cosine. Returns `(concept_id,
/// similarity)` where similarity = 1 − distance (cosine metric on the vec0
/// table). `None` if no concepts exist yet.
fn knn_concept(conn: &rusqlite::Connection, query_vec: &[f32]) -> Result<Option<(i64, f32)>> {
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

fn store_braindump_embedding(
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

fn insert_concept_provenance(
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

fn insert_edge_provenance(
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

fn insert_edge(
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
fn init_type_history(conn: &rusqlite::Connection, edge_id: i64, type_slug: &str) -> Result<()> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
         VALUES (?1, 0, ?2, ?3)",
        params![edge_id, type_slug, created_at],
    )?;
    Ok(())
}

fn find_edge_id(
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

fn insert_merge_suggestion(
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
/// vanish (ADR-0002 / ADR-0010); type-history and suggestions cascade.
fn retract_extraction(conn: &rusqlite::Connection, braindump_id: i64) -> Result<()> {
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
    // Orphan edges first (they reference concepts), then orphan concepts.
    conn.execute(
        "DELETE FROM edges WHERE NOT EXISTS
            (SELECT 1 FROM edge_provenance WHERE edge_id = edges.id)",
        [],
    )?;
    conn.execute(
        "DELETE FROM concepts WHERE NOT EXISTS
            (SELECT 1 FROM concept_provenance WHERE concept_id = concepts.id)",
        [],
    )?;
    Ok(())
}

/// f32 slice → little-endian byte blob, the on-disk format sqlite-vec expects.
pub(crate) fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

// --- read helpers (public; the future GET /graph surface + test seam) ---

/// Load the governed edge-type slugs (the LLM draws from these).
pub async fn ontology_slugs(db: &Db) -> Result<Vec<String>> {
    db.run(ontology_slugs_conn).await
}

fn ontology_slugs_conn(conn: &rusqlite::Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT slug FROM ontology ORDER BY id")?;
    let slugs = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(slugs)
}

pub async fn get_concept(db: &Db, id: i64) -> Result<Option<Concept>> {
    db.run(move |conn| {
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

/// The braindump ids that extracted a concept (ADR-0010 extraction provenance).
pub async fn concept_provenance(db: &Db, concept_id: i64) -> Result<Vec<i64>> {
    db.run(move |conn| {
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

pub async fn find_edge(
    db: &Db,
    source_id: i64,
    original_type: &str,
    target_id: i64,
) -> Result<Option<Edge>> {
    let original_type = original_type.to_string();
    db.run(move |conn| {
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

/// The braindump ids asserting an edge (ADR-0002 `asserted_by`).
pub async fn edge_provenance(db: &Db, edge_id: i64) -> Result<Vec<i64>> {
    db.run(move |conn| {
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

/// The append-only type history of an edge (ADR-0003). Index 0 is the original
/// assertion; the last entry is the current (projected) type.
pub async fn edge_type_history(db: &Db, edge_id: i64) -> Result<Vec<TypeHistoryEntry>> {
    db.run(move |conn| {
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

pub async fn merge_suggestions(db: &Db) -> Result<Vec<MergeSuggestion>> {
    db.run(|conn| {
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

/// Look up a concept id by exact label. Identity is by embedding (ADR-0001),
/// not label, so this is a test/inspection helper — not the identity path.
pub async fn concept_id_for_label(db: &Db, label: &str) -> Result<Option<i64>> {
    let label = label.to_string();
    db.run(move |conn| {
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

/// Whether a braindump-embedding is stored (retrieval backfill, ADR-0004).
pub async fn braindump_embedding_stored(db: &Db, braindump_id: i64) -> Result<bool> {
    db.run(move |conn| {
        let exists: i64 = conn.query_row(
            "SELECT count(*) FROM braindump_embeddings WHERE braindump_id = ?1",
            params![braindump_id],
            |r| r.get(0),
        )?;
        Ok(exists > 0)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::embedding::FakeEmbedding;
    use crate::extractor::{ExtractedConcept, ExtractedEdge};

    /// In-memory Db with vec tables at the fake embedding dim.
    fn test_db() -> Db {
        test_db_dim(FakeEmbedding::default().dim())
    }

    /// In-memory Db with vec tables at a chosen dim (for scripted-embedding
    /// tests that need a specific dimensionality).
    fn test_db_dim(dim: usize) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(dim).unwrap();
        db
    }

    fn fake_embedding() -> FakeEmbedding {
        FakeEmbedding::default()
    }

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

    async fn seed_braindump(db: &Db, text: &str) -> i64 {
        let b = insert_braindump(db, text, text).await.unwrap();
        b.id
    }

    #[tokio::test]
    async fn new_concept_created_with_provenance_and_embedding() {
        let db = test_db();
        let emb = fake_embedding();
        let bd = seed_braindump(&db, "q3 review went off the rails").await;

        let outcome = ingest_extraction(
            &db,
            &emb,
            bd,
            "q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        assert_eq!(outcome.concepts_created, 1);
        assert_eq!(outcome.concepts_accreted, 0);
        let cid = db_concept_id_for_label(&db, "Q3 review").await;
        let concept = get_concept(&db, cid).await.unwrap().unwrap();
        assert_eq!(concept.label, "Q3 review");
        // Extraction provenance (ADR-0010): this braindump extracted it.
        assert_eq!(concept_provenance(&db, cid).await.unwrap(), vec![bd]);
        // Concept-embedding persisted (identity + retrieval seed).
        assert!(concept_embedding_stored(&db, cid).await);
        // Braindump-embedding persisted (retrieval backfill).
        assert!(braindump_embedding_stored(&db, bd).await.unwrap());
    }

    #[tokio::test]
    async fn same_concept_accretes_into_one_node_across_two_braindumps() {
        let db = test_db();
        let emb = fake_embedding();

        let bd1 = seed_braindump(&db, "the q3 review went off the rails").await;
        ingest_extraction(
            &db,
            &emb,
            bd1,
            "the q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(&db, "q3 review is still on my mind").await;
        let outcome = ingest_extraction(
            &db,
            &emb,
            bd2,
            "q3 review is still on my mind",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        // Second extraction accretes to the same node (identical label →
        // identical FakeEmbedding vector → cosine 1.0 > 0.95).
        assert_eq!(outcome.concepts_created, 0, "{outcome:?}");
        assert_eq!(outcome.concepts_accreted, 1, "{outcome:?}");
        assert_eq!(count_concepts(&db).await, 1, "one node, not two");
        let cid = db_concept_id_for_label(&db, "Q3 review").await;
        // Both braindumps in the concept's extraction provenance (ADR-0010).
        let mut prov = concept_provenance(&db, cid).await.unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
    }

    #[tokio::test]
    async fn distinct_concepts_stay_separate() {
        let db = test_db();
        let emb = fake_embedding();

        let bd = seed_braindump(&db, "maria and the q3 launch").await;
        ingest_extraction(
            &db,
            &emb,
            bd,
            "maria and the q3 launch",
            extraction(&["Maria", "Q3 launch"], &[]),
        )
        .await
        .unwrap();

        // No token overlap between "maria" and "q3 launch" in the fake
        // embedding → cosine 0 < floor → two separate concepts.
        assert_eq!(count_concepts(&db).await, 2);
        assert!(db_concept_id_for_label(&db, "Maria").await > 0);
        assert!(db_concept_id_for_label(&db, "Q3 launch").await > 0);
    }

    #[tokio::test]
    async fn edge_accretes_provenance_and_inits_type_history_at_index_zero() {
        let db = test_db();
        let emb = fake_embedding();

        let bd = seed_braindump(&db, "maria endangers the q3 launch").await;
        let outcome = ingest_extraction(
            &db,
            &emb,
            bd,
            "maria endangers the q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        assert_eq!(outcome.edges_created, 1);
        let maria = db_concept_id_for_label(&db, "Maria").await;
        let q3 = db_concept_id_for_label(&db, "Q3 launch").await;
        let edge = find_edge(&db, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");
        assert_eq!(edge.original_type, "endangers");
        // Asserted_by this braindump (ADR-0002).
        assert_eq!(edge_provenance(&db, edge.id).await.unwrap(), vec![bd]);
        // Type history initialized at index 0 = the original assertion
        // (ADR-0003).
        let history = edge_type_history(&db, edge.id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
    }

    #[tokio::test]
    async fn second_braindump_asserting_same_edge_accretes_not_duplicates() {
        let db = test_db();
        let emb = fake_embedding();

        let bd1 = seed_braindump(&db, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            &emb,
            bd1,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(&db, "maria still endangers q3 launch").await;
        let outcome = ingest_extraction(
            &db,
            &emb,
            bd2,
            "maria still endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        // Edge accretes: the second braindump adds to asserted_by, no new edge.
        assert_eq!(outcome.edges_created, 0, "{outcome:?}");
        assert_eq!(outcome.edges_accreted, 1, "{outcome:?}");
        assert_eq!(count_edges(&db).await, 1, "one edge, accreted");
        let maria = db_concept_id_for_label(&db, "Maria").await;
        let q3 = db_concept_id_for_label(&db, "Q3 launch").await;
        let edge = find_edge(&db, maria, "endangers", q3)
            .await
            .unwrap()
            .unwrap();
        let mut prov = edge_provenance(&db, edge.id).await.unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
    }

    #[tokio::test]
    async fn contradictory_edges_coexist_as_separate_typed_edges() {
        // ADR-0002: two braindumps may assert contradictory edges between the
        // same pair; both coexist, each with its own provenance.
        let db = test_db();
        let emb = fake_embedding();

        let bd1 = seed_braindump(&db, "maria helps the q3 launch").await;
        ingest_extraction(
            &db,
            &emb,
            bd1,
            "maria helps the q3 launch",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(&db, "maria endangers the q3 launch").await;
        ingest_extraction(
            &db,
            &emb,
            bd2,
            "maria endangers the q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        assert_eq!(count_edges(&db).await, 2, "contradictory edges coexist");
    }

    #[tokio::test]
    async fn unsanctioned_edge_type_is_rejected() {
        let db = test_db();
        let emb = fake_embedding();
        let bd = seed_braindump(&db, "maria bamboozles q3 launch").await;
        let outcome = ingest_extraction(
            &db,
            &emb,
            bd,
            "maria bamboozles q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "bamboozles", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        assert_eq!(outcome.edges_rejected, 1, "{outcome:?}");
        assert_eq!(outcome.edges_created, 0);
        assert_eq!(count_edges(&db).await, 0, "unsanctioned edge not stored");
    }

    #[tokio::test]
    async fn borderline_match_creates_concept_and_merge_suggestion() {
        // A scripted embedding places the second concept's vector at exactly
        // cosine 0.9 from the first — inside the suggestion band
        // [0.80, 0.95) — so the outcome is deterministic (ADR-0001: borderline
        // → new concept + merge suggestion, not silent accretion).
        let dim = 2;
        let db = test_db_dim(dim);
        let mut emb = ScriptedEmbedding::new(dim);
        emb.set("alpha", vec![1.0, 0.0]);
        // [0.9, sqrt(1 - 0.81)] is unit-length and cosine 0.9 to [1, 0].
        emb.set("alpha variant", vec![0.9, (1.0_f32 - 0.9 * 0.9).sqrt()]);

        let bd1 = seed_braindump(&db, "thinking about alpha").await;
        ingest_extraction(
            &db,
            &emb,
            bd1,
            "thinking about alpha",
            extraction(&["alpha"], &[]),
        )
        .await
        .unwrap();
        let existing = db_concept_id_for_label(&db, "alpha").await;
        assert!(existing > 0);

        let bd2 = seed_braindump(&db, "more on the alpha variant").await;
        let outcome = ingest_extraction(
            &db,
            &emb,
            bd2,
            "more on the alpha variant",
            extraction(&["alpha variant"], &[]),
        )
        .await
        .unwrap();

        assert_eq!(outcome.merge_suggestions, 1, "{outcome:?}");
        assert_eq!(outcome.concepts_created, 1, "{outcome:?}");
        assert_eq!(outcome.concepts_accreted, 0, "{outcome:?}");

        // The borderline concept was created (so edges can reference it) AND a
        // suggestion links it to the near-match for human confirm/reject.
        let new_id = db_concept_id_for_label(&db, "alpha variant").await;
        assert!(new_id > 0, "borderline concept created");
        let suggestions = merge_suggestions(&db).await.unwrap();
        assert_eq!(suggestions.len(), 1, "{suggestions:?}");
        let s = &suggestions[0];
        assert_eq!(s.kind, "concept");
        assert_eq!(s.braindump_id, bd2);
        assert_eq!(s.new_concept_label, "alpha variant");
        assert_eq!(s.new_concept_id, new_id);
        assert_eq!(s.existing_concept_id, existing);
        assert_eq!(s.status, "pending");
        assert!(
            (s.similarity - 0.9).abs() < 1e-5,
            "similarity is the cosine of the match: {}",
            s.similarity
        );
    }

    #[tokio::test]
    async fn edit_retracts_stale_extraction_before_re_accreting() {
        // ADR-0007: re-extraction on edit mutates derived concepts/edges. The
        // old extraction is retracted (provenance dropped, orphan nodes/edges
        // vanish) before the new one accretes — no double-accretion.
        let db = test_db();
        let emb = fake_embedding();

        let bd = seed_braindump(&db, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            &emb,
            bd,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        assert_eq!(count_concepts(&db).await, 2);
        assert_eq!(count_edges(&db).await, 1);

        // Edit: re-extract with a totally different concept set. The old
        // Maria/Q3 concepts vanish (no other braindump asserts them).
        ingest_extraction(
            &db,
            &emb,
            bd,
            "the alpha project",
            extraction(&["Alpha project"], &[]),
        )
        .await
        .unwrap();

        assert_eq!(count_concepts(&db).await, 1, "stale concepts retracted");
        assert_eq!(count_edges(&db).await, 0, "stale edge retracted");
        assert!(db_concept_id_for_label(&db, "Maria").await == 0);
        assert!(db_concept_id_for_label(&db, "Alpha project").await > 0);
        // The braindump's embedding was re-stored (re-embedded on edit).
        assert!(braindump_embedding_stored(&db, bd).await.unwrap());
    }

    #[tokio::test]
    async fn extraction_is_atomic_on_failure() {
        // A non-existent braindump_id violates the edge_provenance FK
        // (braindump_id → braindumps.id). The whole transaction must roll back:
        // no concept, no embedding, no partial state. (foreign_keys is ON.)
        let db = test_db();
        let emb = fake_embedding();
        let ghost_braindump = 9999; // never inserted

        let outcome = ingest_extraction(
            &db,
            &emb,
            ghost_braindump,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;

        assert!(outcome.is_err(), "FK violation must error: {outcome:?}");
        assert_eq!(count_concepts(&db).await, 0, "no partial commit");
        assert_eq!(count_edges(&db).await, 0, "no partial commit");
    }

    #[tokio::test]
    async fn empty_extraction_stores_only_braindump_embedding() {
        let db = test_db();
        let emb = fake_embedding();
        let bd = seed_braindump(&db, "just a feeling").await;

        let outcome =
            ingest_extraction(&db, &emb, bd, "just a feeling", ExtractionResult::default())
                .await
                .unwrap();

        assert_eq!(outcome, IngestOutcome::default());
        assert_eq!(count_concepts(&db).await, 0);
        assert!(braindump_embedding_stored(&db, bd).await.unwrap());
    }

    // --- test helpers ---

    async fn db_concept_id_for_label(db: &Db, label: &str) -> i64 {
        concept_id_for_label(db, label).await.unwrap().unwrap_or(0)
    }

    async fn count_concepts(db: &Db) -> i64 {
        db.run(|conn| Ok(conn.query_row("SELECT count(*) FROM concepts", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    async fn count_edges(db: &Db) -> i64 {
        db.run(|conn| Ok(conn.query_row("SELECT count(*) FROM edges", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    /// Insert a concept with a hand-rolled label + its fake embedding, no
    /// provenance — used to seed a near-match for the borderline test.
    async fn concept_embedding_stored(db: &Db, concept_id: i64) -> bool {
        db.run(move |conn| {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM concept_embeddings WHERE concept_id = ?1",
                params![concept_id],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .await
        .unwrap()
    }

    /// An embedding client with scripted per-text vectors, for tests that need a
    /// controlled cosine (e.g. to land a match in the merge-suggestion band).
    /// Unknown text falls back to a zero vector (the braindump-verbatim
    /// embedding in those tests — its value is irrelevant to the assertion).
    #[derive(Clone)]
    struct ScriptedEmbedding {
        dim: usize,
        vectors: std::collections::HashMap<String, Vec<f32>>,
    }

    impl ScriptedEmbedding {
        fn new(dim: usize) -> Self {
            Self {
                dim,
                vectors: std::collections::HashMap::new(),
            }
        }
        fn set(&mut self, text: &str, vec: Vec<f32>) {
            self.vectors.insert(text.to_string(), vec);
        }
    }

    #[async_trait::async_trait]
    impl EmbeddingClient for ScriptedEmbedding {
        async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
            Ok(self
                .vectors
                .get(text)
                .cloned()
                .unwrap_or_else(|| vec![0.0; self.dim]))
        }
        async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            self.embed_document(text).await
        }
        fn dim(&self) -> usize {
            self.dim
        }
    }
}
