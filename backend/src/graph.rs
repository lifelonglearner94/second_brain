//! The atomic accretion pipeline types + delegating wrappers (issue #6,
//! ADR-0001 / ADR-0002 / ADR-0003 / ADR-0010).
//!
//! The accretion logic (retract → embed-store → identity-resolution → concept
//! accretion → edge accretion → type-history init → provenance) moved behind
//! the [`GraphRepo`] trait in issue #46 and now lives in
//! [`crate::graph_repo::SqliteGraphRepo`]'s trait impl. This module retains
//! the domain types ([`Concept`], [`Edge`], [`MergeSuggestion`],
//! [`IngestOutcome`], [`EdgeProjection`], [`TypeHistoryEntry`]) and the
//! identity thresholds ([`ACCRETION_SIMILARITY`], [`SUGGESTION_FLOOR_SIMILARITY`])
//! that the accretion logic consumes, plus one-line delegating wrappers
//! ([`ingest_extraction`], [`delete_braindump`], [`approve_merge_suggestion`],
//! [`reject_merge_suggestion`]) so existing callers - including the
//! integration tests under `backend/tests/` - keep compiling without taking a
//! `&dyn GraphRepo` directly. #48 removes these wrappers once every caller is
//! migrated.
//!
//! The `pub(crate) *_conn` helpers that used to live here moved into
//! [`crate::graph_repo`] (the Sqlite adapter's home) in #46; `graph.rs` no
//! longer imports or calls any `*_conn` helper.

use serde::Serialize;

use crate::db::Db;
use crate::error::Result;
use crate::extractor::ExtractionResult;
use crate::graph_repo::{GraphRepo, SqliteGraphRepo};
use crate::llm::Llm;

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

/// The accretion pipeline entry point (delegating wrapper, issue #46). Embeds
/// the braindump and each concept label (Gemini), then delegates the atomic
/// accretion (embedding storage, identity resolution, accretion, provenance,
/// type-history init) to [`GraphRepo::ingest_extraction`] on a
/// [`SqliteGraphRepo`].
///
/// The embedding *computation* (network call) runs here; the precomputed
/// vectors are passed into the trait method so the synchronous storage work
/// commits atomically (ADR-0001).
///
/// Idempotent over a braindump: any prior extraction for `braindump_id` is
/// retracted first (concepts/edges losing their last asserter vanish), so this
/// is safe to call on both submit (retracts nothing) and edit (retracts the
/// stale extraction before re-accreting - ADR-0007).
pub async fn ingest_extraction(
    db: &Db,
    user_id: &str,
    llm: &dyn Llm,
    braindump_id: i64,
    verbatim: &str,
    extraction: ExtractionResult,
) -> Result<IngestOutcome> {
    db.ensure_vec_tables(llm.dim())?;
    let braindump_vec = llm.embed_document(verbatim).await?;
    let mut concept_vecs = Vec::with_capacity(extraction.concepts.len());
    for concept in &extraction.concepts {
        concept_vecs.push(llm.embed_document(&concept.label).await?);
    }
    SqliteGraphRepo::new(db.clone())
        .ingest_extraction(
            user_id,
            braindump_id,
            braindump_vec,
            extraction,
            concept_vecs,
        )
        .await
}

// --- read helpers (public; the future GET /graph surface + test seam) ---
//
// These free functions remain as one-line delegators to the [`GraphRepo`]
// trait (issue #45) so existing callers - including the integration tests
// under `backend/tests/` and the unit tests in this module - keep compiling
// without taking a `&dyn GraphRepo` directly. The raw SQL that used to live
// here moved into [`SqliteGraphRepo`]'s trait impl (one source of truth);
// #48 removes these delegators once every caller is migrated to pass the
// repo through.

/// Load the governed edge-type slugs (the LLM draws from these).
pub async fn ontology_slugs(db: &Db, user_id: &str) -> Result<Vec<String>> {
    SqliteGraphRepo::new(db.clone())
        .ontology_slugs(user_id)
        .await
}

pub async fn get_concept(db: &Db, user_id: &str, id: i64) -> Result<Option<Concept>> {
    SqliteGraphRepo::new(db.clone())
        .get_concept(user_id, id)
        .await
}

/// The braindump ids that extracted a concept (ADR-0010 extraction provenance).
pub async fn concept_provenance(db: &Db, user_id: &str, concept_id: i64) -> Result<Vec<i64>> {
    SqliteGraphRepo::new(db.clone())
        .concept_provenance(user_id, concept_id)
        .await
}

pub async fn find_edge(
    db: &Db,
    user_id: &str,
    source_id: i64,
    original_type: &str,
    target_id: i64,
) -> Result<Option<Edge>> {
    SqliteGraphRepo::new(db.clone())
        .find_edge(user_id, source_id, original_type, target_id)
        .await
}

/// The braindump ids asserting an edge (ADR-0002 `asserted_by`).
pub async fn edge_provenance(db: &Db, user_id: &str, edge_id: i64) -> Result<Vec<i64>> {
    SqliteGraphRepo::new(db.clone())
        .edge_provenance(user_id, edge_id)
        .await
}

/// The append-only type history of an edge (ADR-0003). Index 0 is the original
/// assertion; the last entry is the current (projected) type.
pub async fn edge_type_history(
    db: &Db,
    user_id: &str,
    edge_id: i64,
) -> Result<Vec<TypeHistoryEntry>> {
    SqliteGraphRepo::new(db.clone())
        .edge_type_history(user_id, edge_id)
        .await
}

pub async fn merge_suggestions(db: &Db, user_id: &str) -> Result<Vec<MergeSuggestion>> {
    SqliteGraphRepo::new(db.clone())
        .merge_suggestions(user_id)
        .await
}

/// Look up a concept id by exact label. Identity is by embedding (ADR-0001), /// not label, so this is a test/inspection helper - not the identity path.
pub async fn concept_id_for_label(db: &Db, user_id: &str, label: &str) -> Result<Option<i64>> {
    SqliteGraphRepo::new(db.clone())
        .concept_id_for_label(user_id, label)
        .await
}

/// Whether a braindump-embedding is stored (retrieval backfill, ADR-0004).
pub async fn braindump_embedding_stored(db: &Db, user_id: &str, braindump_id: i64) -> Result<bool> {
    SqliteGraphRepo::new(db.clone())
        .braindump_embedding_stored(user_id, braindump_id)
        .await
}

/// An edge paired with its projected current type (ADR-0003) - the last entry
/// of the append-only `edge_type_history`, not a stored field. `original_type`
/// anchors identity (immutable); `current_type` is the read-model projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EdgeProjection {
    pub id: i64,
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub original_type: String,
    pub current_type: String,
    pub created_at: i64,
}

/// Every concept, ordered by id - the full node set for whole-graph reads
/// (issue #27's Global Topology Snapshot).
pub async fn all_concepts(db: &Db, user_id: &str) -> Result<Vec<Concept>> {
    SqliteGraphRepo::new(db.clone()).all_concepts(user_id).await
}

/// Every edge with its projected current type (ADR-0003), ordered by id. The
/// current type is the last `edge_type_history` entry; for a freshly-created
/// edge it equals the original assertion.
pub async fn all_edges_with_current_type(db: &Db, user_id: &str) -> Result<Vec<EdgeProjection>> {
    SqliteGraphRepo::new(db.clone())
        .all_edges_with_current_type(user_id)
        .await
}

// --- issue #7: braindump deletion + merge-suggestion queue (delegating
//     wrappers, issue #46) ---

/// Delete a braindump and cascade through the graph (ADR-0002 / ADR-0007 /
/// ADR-0010). Delegates to [`GraphRepo::delete_braindump`] on a
/// [`SqliteGraphRepo`]. Returns `false` if no braindump with `id` exists.
pub async fn delete_braindump(db: &Db, user_id: &str, braindump_id: i64) -> Result<bool> {
    SqliteGraphRepo::new(db.clone())
        .delete_braindump(user_id, braindump_id)
        .await
}

/// Approve a pending concept merge suggestion (ADR-0001 / ADR-0010): fold the
/// `new_concept_id` into the `existing_concept_id` - union their extraction
/// provenance and repoint edges from the fold concept onto the surviving one,
/// merging duplicate edges by unioning provenance (ADR-0002 accretion). The
/// fold concept and the suggestion are removed. `NotFound` if the suggestion
/// does not exist. Delegates to [`GraphRepo::approve_merge_suggestion`].
pub async fn approve_merge_suggestion(db: &Db, user_id: &str, suggestion_id: i64) -> Result<()> {
    SqliteGraphRepo::new(db.clone())
        .approve_merge_suggestion(user_id, suggestion_id)
        .await
}

/// Reject a pending concept merge suggestion (ADR-0001): keep the two concepts
/// separate and drop the suggestion. `NotFound` if the suggestion does not
/// exist. Delegates to [`GraphRepo::reject_merge_suggestion`].
pub async fn reject_merge_suggestion(db: &Db, user_id: &str, suggestion_id: i64) -> Result<()> {
    SqliteGraphRepo::new(db.clone())
        .reject_merge_suggestion(user_id, suggestion_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::{get_braindump, insert_braindump};
    use crate::db::now_seconds;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;
    use crate::error::Error;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::llm::{FakeLlm, Llm};
    use rusqlite::params;

    /// In-memory Db with vec tables at the fake embedding dim.
    fn test_db() -> Db {
        test_db_dim(FakeLlm::default().dim())
    }

    /// In-memory Db with vec tables at a chosen dim (for scripted-embedding
    /// tests that need a specific dimensionality).
    fn test_db_dim(dim: usize) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(dim).unwrap();
        db
    }

    fn fake_llm() -> FakeLlm {
        FakeLlm::default()
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

    async fn seed_braindump(db: &Db, _user_id: &str, text: &str) -> i64 {
        let b = insert_braindump(db, BOOTSTRAP_ADMIN_USER_ID, text, text)
            .await
            .unwrap();
        b.id
    }

    /// Insert a pending concept merge suggestion directly (the borderline
    /// detection path is covered by the ingest tests; the merge operation is
    /// the unit under test here).
    async fn seed_suggestion(
        db: &Db,
        user_id: &str,
        braindump_id: i64,
        new_concept_id: i64,
        existing_concept_id: i64,
    ) -> i64 {
        let user_id = user_id.to_string();
        db.with_conn(move |conn| {
            let created_at = now_seconds();
            conn.execute(
                "INSERT INTO merge_suggestions
                    (user_id, kind, braindump_id, new_concept_label, new_concept_id,
                     existing_concept_id, similarity, status, created_at)
                 VALUES (?1, 'concept', ?2, 'label', ?3, ?4, 0.9, 'pending', ?5)",
                params![
                    user_id,
                    braindump_id,
                    new_concept_id,
                    existing_concept_id,
                    created_at
                ],
            )?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .unwrap()
    }

    /// Back an edge with a braindump that did not extract its endpoint concepts
    /// (simulates a future chat-inference asserter, ADR-0006 - used to exercise
    /// the endpoint-vanishing cascade, ADR-0010 addendum).
    async fn seed_edge_provenance(db: &Db, _user_id: &str, edge_id: i64, braindump_id: i64) {
        db.with_conn(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO edge_provenance (edge_id, braindump_id)
                 VALUES (?1, ?2)",
                params![edge_id, braindump_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn new_concept_created_with_provenance_and_embedding() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "q3 review went off the rails").await;

        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        assert_eq!(outcome.concepts_created, 1);
        assert_eq!(outcome.concepts_accreted, 0);
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 review").await;
        let concept = get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(concept.label, "Q3 review");
        // Extraction provenance (ADR-0010): this braindump extracted it.
        assert_eq!(
            concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
                .await
                .unwrap(),
            vec![bd]
        );
        // Concept-embedding persisted (identity + retrieval seed).
        assert!(concept_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, cid).await);
        // Braindump-embedding persisted (retrieval backfill).
        assert!(braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn same_concept_accretes_into_one_node_across_two_braindumps() {
        let db = test_db();
        let llm = fake_llm();

        let bd1 = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "the q3 review went off the rails",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "the q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "q3 review is still on my mind",
        )
        .await;
        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "q3 review is still on my mind",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        // Second extraction accretes to the same node (identical label →
        // identical FakeLlm vector → cosine 1.0 > 0.95).
        assert_eq!(outcome.concepts_created, 0, "{outcome:?}");
        assert_eq!(outcome.concepts_accreted, 1, "{outcome:?}");
        assert_eq!(
            count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            1,
            "one node, not two"
        );
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 review").await;
        // Both braindumps in the concept's extraction provenance (ADR-0010).
        let mut prov = concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
    }

    #[tokio::test]
    async fn distinct_concepts_stay_separate() {
        let db = test_db();
        let llm = fake_llm();

        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria and the q3 launch").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria and the q3 launch",
            extraction(&["Maria", "Q3 launch"], &[]),
        )
        .await
        .unwrap();

        // No token overlap between "maria" and "q3 launch" in the fake
        // embedding → cosine 0 < floor → two separate concepts.
        assert_eq!(count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await, 2);
        assert!(db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await > 0);
        assert!(db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await > 0);
    }

    #[tokio::test]
    async fn edge_accretes_provenance_and_inits_type_history_at_index_zero() {
        let db = test_db();
        let llm = fake_llm();

        let bd = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "maria endangers the q3 launch",
        )
        .await;
        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");
        assert_eq!(edge.original_type, "endangers");
        // Asserted_by this braindump (ADR-0002).
        assert_eq!(
            edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap(),
            vec![bd]
        );
        // Type history initialized at index 0 = the original assertion
        // (ADR-0003).
        let history = edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
    }

    #[tokio::test]
    async fn second_braindump_asserting_same_edge_accretes_not_duplicates() {
        let db = test_db();
        let llm = fake_llm();

        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "maria still endangers q3 launch",
        )
        .await;
        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            1,
            "one edge, accreted"
        );
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .unwrap();
        let mut prov = edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
    }

    #[tokio::test]
    async fn contradictory_edges_coexist_as_separate_typed_edges() {
        // ADR-0002: two braindumps may assert contradictory edges between the
        // same pair; both coexist, each with its own provenance.
        let db = test_db();
        let llm = fake_llm();

        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria helps the q3 launch").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria helps the q3 launch",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await
        .unwrap();

        let bd2 = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "maria endangers the q3 launch",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "maria endangers the q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            2,
            "contradictory edges coexist"
        );
    }

    #[tokio::test]
    async fn unsanctioned_edge_type_is_rejected() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria bamboozles q3 launch").await;
        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            0,
            "unsanctioned edge not stored"
        );
    }

    #[tokio::test]
    async fn borderline_match_creates_concept_and_merge_suggestion() {
        // A scripted embedding places the second concept's vector at exactly
        // cosine 0.9 from the first - inside the suggestion band
        // [0.80, 0.95) - so the outcome is deterministic (ADR-0001: borderline
        // → new concept + merge suggestion, not silent accretion).
        let dim = 2;
        let db = test_db_dim(dim);
        let mut llm = ScriptedLlm::new(dim);
        llm.set("alpha", vec![1.0, 0.0]);
        // [0.9, sqrt(1 - 0.81)] is unit-length and cosine 0.9 to [1, 0].
        llm.set("alpha variant", vec![0.9, (1.0_f32 - 0.9 * 0.9).sqrt()]);

        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "thinking about alpha").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "thinking about alpha",
            extraction(&["alpha"], &[]),
        )
        .await
        .unwrap();
        let existing = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "alpha").await;
        assert!(existing > 0);

        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "more on the alpha variant").await;
        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        let new_id = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "alpha variant").await;
        assert!(new_id > 0, "borderline concept created");
        let suggestions = merge_suggestions(&db, BOOTSTRAP_ADMIN_USER_ID)
            .await
            .unwrap();
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
        // vanish) before the new one accretes - no double-accretion.
        let db = test_db();
        let llm = fake_llm();

        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        assert_eq!(count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await, 2);
        assert_eq!(count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await, 1);

        // Edit: re-extract with a totally different concept set. The old
        // Maria/Q3 concepts vanish (no other braindump asserts them).
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "the alpha project",
            extraction(&["Alpha project"], &[]),
        )
        .await
        .unwrap();

        assert_eq!(
            count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            1,
            "stale concepts retracted"
        );
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            0,
            "stale edge retracted"
        );
        assert!(db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await == 0);
        assert!(db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Alpha project").await > 0);
        // The braindump's embedding was re-stored (re-embedded on edit).
        assert!(braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn extraction_is_atomic_on_failure() {
        // A non-existent braindump_id violates the edge_provenance FK
        // (braindump_id → braindumps.id). The whole transaction must roll back:
        // no concept, no embedding, no partial state. (foreign_keys is ON.)
        let db = test_db();
        let llm = fake_llm();
        let ghost_braindump = 9999; // never inserted

        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            ghost_braindump,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;

        assert!(outcome.is_err(), "FK violation must error: {outcome:?}");
        assert_eq!(
            count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            0,
            "no partial commit"
        );
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            0,
            "no partial commit"
        );
    }

    #[tokio::test]
    async fn empty_extraction_stores_only_braindump_embedding() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "just a feeling").await;

        let outcome = ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "just a feeling",
            ExtractionResult::default(),
        )
        .await
        .unwrap();

        assert_eq!(outcome, IngestOutcome::default());
        assert_eq!(count_concepts(&db, BOOTSTRAP_ADMIN_USER_ID).await, 0);
        assert!(braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap());
    }

    // --- issue #7: braindump deletion with provenance cascade (ADR-0002/0007/0010) ---

    #[tokio::test]
    async fn delete_braindump_drops_extraction_provenance_and_vanishes_on_last_extractor() {
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "thinking about q3").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "q3 again").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "thinking about q3",
            extraction(&["Q3"], &[]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "q3 again",
            extraction(&["Q3"], &[]),
        )
        .await
        .unwrap();
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3").await;
        let mut prov = concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);

        // Delete bd1: Q3 still extracted by bd2 → survives, provenance = [bd2].
        assert!(
            delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd1)
                .await
                .unwrap(),
            "deleting an existing braindump reports true"
        );
        assert_eq!(
            concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
                .await
                .unwrap(),
            vec![bd2]
        );
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
                .await
                .unwrap()
                .is_some(),
            "concept survives while another braindump extracts it"
        );
        assert!(
            get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd1)
                .await
                .unwrap()
                .is_none(),
            "braindump row removed"
        );

        // Delete bd2: Q3's last extractor gone → concept vanishes (ADR-0010).
        assert!(delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd2)
            .await
            .unwrap());
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
                .await
                .unwrap()
                .is_none(),
            "concept vanishes when its last extracting braindump is deleted"
        );
    }

    #[tokio::test]
    async fn delete_braindump_drops_edge_provenance_and_vanishes_on_last_asserter() {
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria still endangers q3").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "maria still endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");

        // Delete bd1: edge still asserted by bd2 → survives (ADR-0002).
        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd1)
            .await
            .unwrap();
        assert_eq!(
            edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap(),
            vec![bd2]
        );
        assert!(
            find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
                .await
                .unwrap()
                .is_some(),
            "edge survives while another braindump asserts it"
        );

        // Delete bd2: last asserter gone → edge vanishes (ADR-0002).
        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd2)
            .await
            .unwrap();
        assert!(
            find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
                .await
                .unwrap()
                .is_none(),
            "edge vanishes when its last asserter is deleted"
        );
    }

    #[tokio::test]
    async fn delete_braindump_cascade_deletes_edge_when_endpoint_concept_vanishes() {
        // ADR-0010 addendum: an edge whose endpoint concept vanishes is
        // cascade-deleted, even if another asserter still backs it (a future
        // chat inference may assert an edge without extracting the endpoint -
        // ADR-0006). An edge with a missing endpoint is meaningless.
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria something").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        // bd2 extracts only Maria, so Q3's sole extractor is bd1.
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "maria something",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");
        // Simulate a non-extracting asserter (future chat inference, ADR-0006)
        // backing the edge, so it still has provenance after bd1 is removed.
        seed_edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id, bd2).await;
        let mut prov = edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);

        // Delete bd1: Q3's only extractor → Q3 vanishes. The edge still has bd2
        // as an asserter, but its endpoint (Q3) is gone → cascade-deleted.
        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd1)
            .await
            .unwrap();
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, q3)
                .await
                .unwrap()
                .is_none(),
            "Q3 vanishes: sole extractor deleted"
        );
        assert!(
            find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
                .await
                .unwrap()
                .is_none(),
            "edge cascade-deleted: endpoint concept vanished"
        );
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
                .await
                .unwrap()
                .is_some(),
            "Maria survives: bd2 still extracts it"
        );
    }

    #[tokio::test]
    async fn delete_braindump_removes_row_and_braindump_embedding() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        assert!(braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap());

        assert!(delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap());
        assert!(
            get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
                .await
                .unwrap()
                .is_none(),
            "braindump row removed"
        );
        assert!(
            !braindump_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
                .await
                .unwrap(),
            "braindump embedding removed"
        );
    }

    #[tokio::test]
    async fn delete_braindump_cleans_concept_embeddings_for_vanished_concepts() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        assert!(concept_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, cid).await);

        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap();
        assert!(
            !concept_embedding_stored(&db, BOOTSTRAP_ADMIN_USER_ID, cid).await,
            "vanished concept's embedding cleaned (no orphan in KNN)"
        );
    }

    #[tokio::test]
    async fn delete_missing_braindump_returns_false() {
        let db = test_db();
        assert!(
            !delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, 9999)
                .await
                .unwrap(),
            "deleting a non-existent braindump reports false"
        );
    }

    // --- issue #28: tombstone log for delta-sync deletions ---

    #[tokio::test]
    async fn delete_braindump_writes_tombstone_for_vanished_concept() {
        // When a concept vanishes (its last extracting braindump is deleted),
        // a 'concept' tombstone row is appended so delta sync can report the
        // deletion (ADR-0010; the cascade deletes the row outright otherwise).
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        assert!(cid > 0);

        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap();

        let tombstoned = tombstoned_concept_ids(&db, BOOTSTRAP_ADMIN_USER_ID).await;
        assert!(
            tombstoned.contains(&cid),
            "vanished concept {cid} must be tombstoned: {tombstoned:?}"
        );
    }

    #[tokio::test]
    async fn delete_braindump_writes_tombstone_for_vanished_edge() {
        // When an edge vanishes (its last asserter is deleted), an 'edge'
        // tombstone row is appended so delta sync can report the deletion.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");

        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap();

        let tombstoned = tombstoned_edge_ids(&db, BOOTSTRAP_ADMIN_USER_ID).await;
        assert!(
            tombstoned.contains(&edge.id),
            "vanished edge {} must be tombstoned: {tombstoned:?}",
            edge.id
        );
    }

    #[tokio::test]
    async fn delete_braindump_leaves_no_tombstone_when_concept_survives() {
        // A concept that still has an extracting braindump after the cascade
        // must NOT be tombstoned - only vanished rows are.
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "q3 one").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "q3 two").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "q3 one",
            extraction(&["Q3"], &[]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "q3 two",
            extraction(&["Q3"], &[]),
        )
        .await
        .unwrap();
        let cid = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3").await;

        // Delete bd1: Q3 still extracted by bd2 → survives, no tombstone.
        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd1)
            .await
            .unwrap();
        assert!(
            !tombstoned_concept_ids(&db, BOOTSTRAP_ADMIN_USER_ID)
                .await
                .contains(&cid),
            "surviving concept must not be tombstoned"
        );
        assert!(get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, cid)
            .await
            .unwrap()
            .is_some());

        // Delete bd2: now Q3 vanishes → tombstone.
        delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd2)
            .await
            .unwrap();
        assert!(
            tombstoned_concept_ids(&db, BOOTSTRAP_ADMIN_USER_ID)
                .await
                .contains(&cid),
            "vanished concept must be tombstoned"
        );
    }

    async fn tombstoned_concept_ids(db: &Db, _user_id: &str) -> Vec<i64> {
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT entity_id FROM graph_tombstones WHERE kind = 'concept' ORDER BY entity_id",
            )?;
            let ids = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })
        .await
        .unwrap()
    }

    async fn tombstoned_edge_ids(db: &Db, _user_id: &str) -> Vec<i64> {
        db.with_conn(|conn| {
            let mut stmt = conn.prepare(
                "SELECT entity_id FROM graph_tombstones WHERE kind = 'edge' ORDER BY entity_id",
            )?;
            let ids = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(ids)
        })
        .await
        .unwrap()
    }

    // --- issue #7: concept merge-suggestion queue (ADR-0001/0002/0010) ---

    #[tokio::test]
    async fn approve_merge_unions_extraction_provenance_and_drops_fold_concept() {
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "beta").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "beta",
            extraction(&["Beta"], &[]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let beta = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta").await;
        let suggestion = seed_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, bd2, beta, maria).await;

        approve_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, suggestion)
            .await
            .unwrap();

        // The keeper (maria) carries the fold concept's extraction provenance
        // (ADR-0010: a merged concept's provenance is the union of its members').
        let mut prov = concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
        // The fold concept (beta) is gone.
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, beta)
                .await
                .unwrap()
                .is_none(),
            "fold concept deleted on approve"
        );
        // The suggestion is consumed.
        assert!(
            !merge_suggestions(&db, BOOTSTRAP_ADMIN_USER_ID)
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == suggestion),
            "approved suggestion dropped from the queue"
        );
    }

    #[tokio::test]
    async fn approve_merge_folds_edges_onto_surviving_concept() {
        // ADR-0002 consequence: merging folds edges from both concepts onto the
        // merged node; contradictory edges (different type) coexist rather than
        // being silently resolved.
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria endangers q3").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "beta helps q3").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria endangers q3",
            extraction(&["Maria", "Q3"], &[("Maria", "endangers", "Q3")]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "beta helps q3",
            extraction(&["Beta", "Q3"], &[("Beta", "helps", "Q3")]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let beta = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3").await;
        let suggestion = seed_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, bd2, beta, maria).await;

        approve_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, suggestion)
            .await
            .unwrap();

        // Beta's edge (Beta→Q3[helps]) folded onto Maria → Maria→Q3[helps].
        let folded = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "helps", q3)
            .await
            .unwrap()
            .expect("folded edge present");
        assert_eq!(
            edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, folded.id)
                .await
                .unwrap(),
            vec![bd2]
        );
        // Maria's own edge (endangers) still present - contradictory edges coexist.
        assert!(
            find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
                .await
                .unwrap()
                .is_some(),
            "pre-existing edge preserved"
        );
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            2,
            "two edges, both now Maria→Q3"
        );
        assert!(get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, beta)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn approve_merge_unions_provenance_when_duplicate_edges_collide() {
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria helps q3").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "beta helps q3").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria helps q3",
            extraction(&["Maria", "Q3"], &[("Maria", "helps", "Q3")]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "beta helps q3",
            extraction(&["Beta", "Q3"], &[("Beta", "helps", "Q3")]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let beta = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta").await;
        let q3 = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3").await;
        let suggestion = seed_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, bd2, beta, maria).await;

        approve_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, suggestion)
            .await
            .unwrap();

        // Both asserted →Q3[helps]; after fold they collide on (Maria, helps, Q3)
        // → one edge, provenance unioned (ADR-0002 accretion).
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "helps", q3)
            .await
            .unwrap()
            .expect("merged edge present");
        let mut prov = edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        prov.sort_unstable();
        assert_eq!(prov, vec![bd1, bd2]);
        assert_eq!(
            count_edges(&db, BOOTSTRAP_ADMIN_USER_ID).await,
            1,
            "duplicate edges merged into one"
        );
        assert!(get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, beta)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn reject_merge_keeps_concepts_separate_and_drops_suggestion() {
        let db = test_db();
        let llm = fake_llm();
        let bd1 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "maria").await;
        let bd2 = seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "beta").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "maria",
            extraction(&["Maria"], &[]),
        )
        .await
        .unwrap();
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "beta",
            extraction(&["Beta"], &[]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria").await;
        let beta = db_concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Beta").await;
        let suggestion = seed_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, bd2, beta, maria).await;

        reject_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, suggestion)
            .await
            .unwrap();

        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
                .await
                .unwrap()
                .is_some(),
            "keeper survives reject"
        );
        assert!(
            get_concept(&db, BOOTSTRAP_ADMIN_USER_ID, beta)
                .await
                .unwrap()
                .is_some(),
            "fold concept survives reject"
        );
        assert_eq!(
            concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
                .await
                .unwrap(),
            vec![bd1],
            "provenance unchanged on reject"
        );
        assert!(
            !merge_suggestions(&db, BOOTSTRAP_ADMIN_USER_ID)
                .await
                .unwrap()
                .iter()
                .any(|s| s.id == suggestion),
            "rejected suggestion dropped from the queue"
        );
    }

    #[tokio::test]
    async fn approve_missing_suggestion_is_not_found() {
        let db = test_db();
        let result = approve_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, 9999).await;
        assert!(
            matches!(result, Err(Error::NotFound(_))),
            "approving a missing suggestion is NotFound: {result:?}"
        );
    }

    #[tokio::test]
    async fn reject_missing_suggestion_is_not_found() {
        let db = test_db();
        let result = reject_merge_suggestion(&db, BOOTSTRAP_ADMIN_USER_ID, 9999).await;
        assert!(
            matches!(result, Err(Error::NotFound(_))),
            "rejecting a missing suggestion is NotFound: {result:?}"
        );
    }

    // --- issue #36: shared graph-mutation vocabulary (characterization) ---

    // --- test helpers ---

    async fn db_concept_id_for_label(db: &Db, _user_id: &str, label: &str) -> i64 {
        concept_id_for_label(db, BOOTSTRAP_ADMIN_USER_ID, label)
            .await
            .unwrap()
            .unwrap_or(0)
    }

    async fn count_concepts(db: &Db, _user_id: &str) -> i64 {
        db.with_conn(|conn| Ok(conn.query_row("SELECT count(*) FROM concepts", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    async fn count_edges(db: &Db, _user_id: &str) -> i64 {
        db.with_conn(|conn| Ok(conn.query_row("SELECT count(*) FROM edges", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    /// Insert a concept with a hand-rolled label + its fake embedding, no
    /// provenance - used to seed a near-match for the borderline test.
    async fn concept_embedding_stored(db: &Db, _user_id: &str, concept_id: i64) -> bool {
        db.with_conn(move |conn| {
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

    /// An LLM with scripted per-text embedding vectors, for tests that need a
    /// controlled cosine (e.g. to land a match in the merge-suggestion band).
    /// Unknown text falls back to a zero vector (the braindump-verbatim
    /// embedding in those tests - its value is irrelevant to the assertion).
    /// The non-embedding methods are unused stubs - graph tests only drive
    /// `ingest_extraction`, which touches `embed_document`/`dim`.
    #[derive(Clone)]
    struct ScriptedLlm {
        dim: usize,
        vectors: std::collections::HashMap<String, Vec<f32>>,
    }

    impl ScriptedLlm {
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
    impl Llm for ScriptedLlm {
        async fn clean(&self, verbatim: &str) -> Result<String> {
            Ok(verbatim.trim().to_string())
        }
        async fn generate_pinned(&self, _system: &str, user: &str) -> Result<String> {
            Ok(user.to_string())
        }
        async fn synthesize(&self, _system: &str, _user: &str) -> Result<String> {
            Ok("ScriptedLlm::synthesize (unused by graph tests)".to_string())
        }
        async fn extract(
            &self,
            _verbatim: &str,
            _ontology_slugs: &[String],
        ) -> Result<ExtractionResult> {
            Ok(ExtractionResult::default())
        }
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
