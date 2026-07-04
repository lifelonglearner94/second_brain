//! Chat write-back, structural mode (issue #11, ADR-0006).
//!
//! Chat is not only a read surface (ADR-0005) but a governed write-back
//! surface. When chat traces an existing multi-hop edge path, it may *propose*
//! a direct edge summarizing it — a Structural Inference. The proposal is
//! graph-backed (deterministic, low-risk): its evidence is a traversable edge
//! path, captured verbatim in the proposal row.
//!
//! The inference-claim is ALWAYS human-gated — no auto-endorse. Endorsing an
//! LLM deduction is the highest-stakes graph mutation (it can drift the brain
//! toward the LLM's worldview), so the propose→HITL→endorse pattern applies
//! with no tolerance threshold. A proposal starts `pending`; only the explicit
//! endorse endpoint flips it to `endorsed` and persists the edge.
//!
//! On endorsement the edge persists (or accretes, if it already exists) with
//! provenance recorded in `edge_inference_provenance` as
//! `asserted_by: [Chat_Inference_ID, mode: structural]` — origin-typed so user
//! thoughts (braindump provenance, ADR-0002) and LLM deductions stay
//! distinguishable. Structural inferences carry NO Thematic Snapshot
//! (ADR-0009): their evidence is the graph itself, always present, recorded in
//! `evidence_path`.
//!
//! Thematic mode (ADR-0006, issue #13) is the riskier, non-graph-backed sibling
//! and is out of scope here; the `mode` column and the shared queue surface
//! are sized for it, but only `structural` is emitted by this slice.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{now_seconds, Db};
use crate::error::{Error, Result};

/// Origin tag for a structural inference proposal (ADR-0006). The
/// graph-backed, deterministic mode — "the graph supports this; I'm labeling
/// existing structure."
pub const STRUCTURAL_MODE: &str = "structural_inference";

/// Proposal lifecycle statuses. `pending` is the queue; `endorsed`/`rejected`
/// are the two HITL termini. There is no auto-endorse.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_ENDORSED: &str = "endorsed";
pub const STATUS_REJECTED: &str = "rejected";

/// One hop of the traversable edge path that backs a structural inference
/// (ADR-0006). The proposal's evidence is this path; on endorse the direct
/// edge `source —[proposed_type]→ target` summarizes it. Serialized as JSON
/// into `chat_inference_proposals.evidence_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceEdge {
    pub source_concept_id: i64,
    pub edge_type: String,
    pub target_concept_id: i64,
}

/// A pending or resolved chat-inference proposal (read model). `status` is
/// `pending`, `endorsed`, or `rejected`. `evidence_path` is the traversable
/// edge path that backs a structural proposal (empty for thematic — issue #13).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChatInferenceProposal {
    pub id: i64,
    pub mode: String,
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub proposed_type: String,
    pub evidence_path: Vec<EvidenceEdge>,
    pub rationale: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

/// One chat-inference assertion backing an edge (ADR-0006 origin-typed
/// provenance). `mode` is `structural_inference` or `thematic_inference`. The
/// braindump-origin half of `asserted_by` lives in `edge_provenance`
/// (ADR-0002); this is the chat-inference half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InferenceAssertion {
    pub chat_inference_id: i64,
    pub mode: String,
}

/// Propose a structural inference (ADR-0006 structural mode): a direct edge
/// `source —[proposed_type]→ target` summarizing a real, traversable
/// multi-hop edge path. The proposal enters the queue as `pending` — it is
/// NEVER auto-endorsed. On a later endorse the edge persists with
/// `asserted_by: [this proposal's id, mode: structural_inference]`.
///
/// Validation enforces the structural guarantee (graph-backed, deterministic):
/// the `evidence_path` must be non-empty, connected (each hop's target is the
/// next hop's source, the first hop's source is `source_concept_id`, the last
/// hop's target is `target_concept_id`), and every hop must be a real edge in
/// the graph wearing the stated type as its current projected type
/// (ADR-0003). The `proposed_type` must be a governed ontology slug
/// (ADR-0002: the LLM never invents a type); an unsanctioned type is rejected
/// and the caller is directed to the ontology governance queue.
pub async fn propose_structural_inference(
    db: &Db,
    source_concept_id: i64,
    target_concept_id: i64,
    proposed_type: &str,
    evidence_path: Vec<EvidenceEdge>,
    rationale: Option<&str>,
) -> Result<ChatInferenceProposal> {
    let proposed_type = proposed_type.trim().to_string();
    if proposed_type.is_empty() {
        return Err(Error::BadRequest("proposed_type must be non-empty".into()));
    }
    validate_path(&evidence_path, source_concept_id, target_concept_id)?;
    let rationale = rationale
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let evidence_json = serde_json::to_string(&evidence_path)
        .map_err(|e| Error::internal(format!("encode evidence_path: {e}")))?;

    let created_at = now_seconds();
    let proposal = db
        .run(move |conn| {
            // The proposed type must be a governed ontology slug (ADR-0002).
            // An unsanctioned type routes to the ontology governance queue —
            // the caller must propose the type via `POST /ontology/propose`
            // and re-propose the inference once it is approved.
            if !ontology_slug_exists_conn(conn, &proposed_type)? {
                return Err(Error::BadRequest(format!(
                    "proposed type `{proposed_type}` is not in the ontology; \
                     propose it via POST /ontology/propose and re-propose the \
                     inference once approved"
                )));
            }
            // The structural guarantee: every hop must be a real edge wearing
            // the stated type as its current projected type. This is what
            // makes a structural inference graph-backed rather than
            // hallucinated.
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
                     evidence_path, rationale, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
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
            })
        })
        .await?;
    Ok(proposal)
}

/// Validate the evidence path is non-empty, connected, and spans
/// `source_concept_id` → `target_concept_id`. Pure (no I/O) so it is
/// hermetically testable. Traversability (each hop is a real edge) is checked
/// against the DB in [`propose_structural_inference`].
fn validate_path(
    path: &[EvidenceEdge],
    source_concept_id: i64,
    target_concept_id: i64,
) -> Result<()> {
    if path.is_empty() {
        return Err(Error::BadRequest(
            "evidence_path must be non-empty — a structural inference summarizes \
             a real multi-hop path"
                .into(),
        ));
    }
    if path[0].source_concept_id != source_concept_id {
        return Err(Error::BadRequest(format!(
            "evidence_path must start at source_concept_id {source_concept_id}; \
             first hop starts at {}",
            path[0].source_concept_id
        )));
    }
    if path[path.len() - 1].target_concept_id != target_concept_id {
        return Err(Error::BadRequest(format!(
            "evidence_path must end at target_concept_id {target_concept_id}; \
             last hop ends at {}",
            path[path.len() - 1].target_concept_id
        )));
    }
    for window in path.windows(2) {
        if window[0].target_concept_id != window[1].source_concept_id {
            return Err(Error::BadRequest(format!(
                "evidence_path is not connected: hop ends at {} but next hop \
                 starts at {}",
                window[0].target_concept_id, window[1].source_concept_id
            )));
        }
    }
    Ok(())
}

/// Endorse a pending structural inference proposal (ADR-0006): persist the
/// direct edge `source —[proposed_type]→ target` and record the proposal as
/// its chat-inference asserter. The edge accretes (ADR-0002): if the direct
/// edge already exists — asserted by a braindump, or by an earlier endorsed
/// inference — the new assertion is added to its provenance rather than
/// duplicated. Type history is initialised at index 0 for a newly-created
/// edge (ADR-0003).
///
/// `NotFound` if the proposal does not exist; `Conflict` if it is not
/// `pending` (already endorsed or rejected — endorsement is immutable, no
/// second chance). Returns the refreshed proposal.
pub async fn endorse_inference_proposal(db: &Db, id: i64) -> Result<ChatInferenceProposal> {
    let proposal = get_inference_proposal(db, id)
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
    db.run(move |conn| {
        conn.execute_batch("BEGIN")?;
        match (|| -> Result<()> {
            let edge_id =
                if let Some(eid) = find_edge_id_conn(conn, source, &proposed_type, target)? {
                    eid
                } else {
                    let eid = insert_edge_conn(conn, source, target, &proposed_type)?;
                    init_type_history_conn(conn, eid, &proposed_type)?;
                    eid
                };
            insert_inference_provenance_conn(conn, edge_id, id, STRUCTURAL_MODE)?;
            let now = now_seconds();
            conn.execute(
                "UPDATE chat_inference_proposals
                 SET status = ?1, resolved_at = ?2 WHERE id = ?3",
                params![STATUS_ENDORSED, now, id],
            )?;
            Ok(())
        })() {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    })
    .await?;
    get_inference_proposal(db, id)
        .await?
        .ok_or_else(|| Error::internal("proposal vanished after endorse"))
}

/// Reject a pending structural inference proposal (ADR-0006): keep the graph
/// untouched and mark the proposal `rejected`. No edge is persisted — the
/// inference-claim never enters the graph. `NotFound` if the proposal does
/// not exist; `Conflict` if it is not `pending`. Returns the refreshed
/// proposal.
pub async fn reject_inference_proposal(db: &Db, id: i64) -> Result<ChatInferenceProposal> {
    let now = now_seconds();
    let updated = db
        .run(move |conn| {
            Ok(conn.execute(
                "UPDATE chat_inference_proposals
                 SET status = ?1, resolved_at = ?2
                 WHERE id = ?3 AND status = ?4",
                params![STATUS_REJECTED, now, id, STATUS_PENDING],
            )?)
        })
        .await?;
    if updated == 0 {
        match get_inference_proposal(db, id).await? {
            None => Err(Error::NotFound(format!(
                "chat inference proposal {id} not found"
            ))),
            Some(p) => Err(Error::Conflict(format!(
                "proposal {id} is `{}`, not `pending` — cannot reject",
                p.status
            ))),
        }
    } else {
        get_inference_proposal(db, id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after reject"))
    }
}

/// List all chat-inference proposals, oldest first.
pub async fn list_inference_proposals(db: &Db) -> Result<Vec<ChatInferenceProposal>> {
    db.run(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, mode, source_concept_id, target_concept_id, proposed_type,
                    evidence_path, rationale, status, created_at, resolved_at
             FROM chat_inference_proposals ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], row_to_proposal)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    })
    .await
}

/// Look up a single proposal by id. `None` if no row matches.
pub async fn get_inference_proposal(db: &Db, id: i64) -> Result<Option<ChatInferenceProposal>> {
    db.run(move |conn| {
        let row = conn
            .query_row(
                "SELECT id, mode, source_concept_id, target_concept_id, proposed_type,
                        evidence_path, rationale, status, created_at, resolved_at
                 FROM chat_inference_proposals WHERE id = ?1",
                params![id],
                row_to_proposal,
            )
            .optional()?;
        Ok(row)
    })
    .await
}

fn row_to_proposal(r: &rusqlite::Row) -> rusqlite::Result<ChatInferenceProposal> {
    let evidence_json: String = r.get(5)?;
    let evidence_path: Vec<EvidenceEdge> = serde_json::from_str(&evidence_json).unwrap_or_default();
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
    })
}

/// The chat-inference assertions backing an edge (ADR-0006 origin-typed
/// provenance). The braindump-origin half of `asserted_by` lives in
/// `edge_provenance` (ADR-0002); this is the chat-inference half. An edge's
/// full asserter list is the union of both.
pub async fn edge_inference_asserted_by(db: &Db, edge_id: i64) -> Result<Vec<InferenceAssertion>> {
    db.run(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT chat_inference_id, mode FROM edge_inference_provenance
             WHERE edge_id = ?1 ORDER BY chat_inference_id",
        )?;
        let rows = stmt
            .query_map(params![edge_id], |r| {
                Ok(InferenceAssertion {
                    chat_inference_id: r.get(0)?,
                    mode: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    })
    .await
}

// --- connection-scoped helpers (shared with graph.rs-style internals) ---

fn ontology_slug_exists_conn(conn: &rusqlite::Connection, slug: &str) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ontology WHERE slug = ?1",
        params![slug],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether an edge `source —[type]→ target` exists in the graph wearing
/// `type` as its current projected type (the last entry of its append-only
/// type history, ADR-0003). This is the traversability check for a structural
/// inference's evidence path.
fn edge_exists_with_current_type_conn(
    conn: &rusqlite::Connection,
    source_id: i64,
    type_slug: &str,
    target_id: i64,
) -> Result<bool> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM edges e
             WHERE e.source_concept_id = ?1
               AND e.target_concept_id = ?2
               AND (
                   SELECT type_slug FROM edge_type_history
                   WHERE edge_id = e.id ORDER BY seq_index DESC LIMIT 1
               ) = ?3",
            params![source_id, target_id, type_slug],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

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

fn insert_inference_provenance_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    chat_inference_id: i64,
    mode: &str,
) -> Result<()> {
    let created_at = now_seconds();
    conn.execute(
        "INSERT OR IGNORE INTO edge_inference_provenance
            (edge_id, chat_inference_id, mode, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![edge_id, chat_inference_id, mode, created_at],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::embedding::{EmbeddingClient, FakeEmbedding};
    use crate::error::Error;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::ingest_extraction;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeEmbedding::default().dim())
            .unwrap();
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

    async fn concept_id(db: &Db, label: &str) -> i64 {
        crate::graph::concept_id_for_label(db, label)
            .await
            .unwrap()
            .unwrap()
    }

    /// Seed the canonical ADR-0006 structural path:
    /// `Maria —[endangers]→ Q3 launch —[depends_on]→ Beta release`.
    /// Returns the three concept ids in that order.
    async fn seed_path(db: &Db) -> (i64, i64, i64) {
        let emb = fake_embedding();
        let bd = seed_braindump(db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            db,
            &emb,
            bd,
            "maria endangers q3 which beta depends on",
            extraction(
                &["Maria", "Q3 launch", "Beta release"],
                &[
                    ("Maria", "endangers", "Q3 launch"),
                    ("Q3 launch", "depends_on", "Beta release"),
                ],
            ),
        )
        .await
        .unwrap();
        let maria = concept_id(db, "Maria").await;
        let q3 = concept_id(db, "Q3 launch").await;
        let beta = concept_id(db, "Beta release").await;
        (maria, q3, beta)
    }

    fn hop(s: i64, t: &str, tg: i64) -> EvidenceEdge {
        EvidenceEdge {
            source_concept_id: s,
            edge_type: t.to_string(),
            target_concept_id: tg,
        }
    }

    // --- validate_path (pure) ---

    #[test]
    fn empty_path_is_rejected() {
        let err = validate_path(&[], 1, 2).unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[test]
    fn path_must_start_at_source() {
        let err = validate_path(&[hop(5, "endangers", 6)], 1, 6).unwrap_err();
        assert!(err.to_string().contains("start"), "{err:?}");
    }

    #[test]
    fn path_must_end_at_target() {
        let err = validate_path(&[hop(1, "endangers", 6)], 1, 2).unwrap_err();
        assert!(err.to_string().contains("end"), "{err:?}");
    }

    #[test]
    fn disconnected_path_is_rejected() {
        let err =
            validate_path(&[hop(1, "endangers", 6), hop(7, "depends_on", 2)], 1, 2).unwrap_err();
        assert!(err.to_string().contains("not connected"), "{err:?}");
    }

    #[test]
    fn connected_path_spanning_source_to_target_is_valid() {
        validate_path(&[hop(1, "endangers", 6), hop(6, "depends_on", 2)], 1, 2).unwrap();
    }

    // --- propose_structural_inference ---

    #[tokio::test]
    async fn propose_with_traversable_path_creates_pending_proposal_and_no_edge() {
        // ADR-0006: the proposal enters the queue pending. No auto-endorse —
        // no edge is persisted yet.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;

        let proposal = propose_structural_inference(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
            Some("Maria endangers the launch the beta depends on"),
        )
        .await
        .unwrap();

        assert_eq!(proposal.mode, STRUCTURAL_MODE);
        assert_eq!(proposal.status, STATUS_PENDING);
        assert_eq!(proposal.source_concept_id, maria);
        assert_eq!(proposal.target_concept_id, beta);
        assert_eq!(proposal.proposed_type, "endangers");
        assert_eq!(
            proposal.evidence_path,
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)]
        );
        assert!(proposal.rationale.is_some());
        assert!(proposal.resolved_at.is_none());

        // No edge Maria —[endangers]→ Beta release exists yet — the claim is
        // not endorsed, so the graph is untouched.
        assert!(
            crate::graph::find_edge(&db, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on a pending proposal (no auto-endorse)"
        );
        assert!(
            edge_inference_asserted_by(&db, 9999)
                .await
                .unwrap()
                .is_empty(),
            "no inference provenance written on a pending proposal"
        );
    }

    #[tokio::test]
    async fn propose_with_non_traversable_path_is_rejected() {
        // The structural guarantee: the path must be real. A hop that is not
        // an edge in the graph is rejected — no proposal is created.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;

        let err = propose_structural_inference(
            &db,
            maria,
            beta,
            "endangers",
            // Maria —[helps]→ Q3 does not exist (the real edge is `endangers`).
            vec![hop(maria, "helps", q3), hop(q3, "depends_on", beta)],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(
            err.to_string().contains("not a traversable edge"),
            "{err:?}"
        );
        assert!(
            list_inference_proposals(&db).await.unwrap().is_empty(),
            "no proposal created for a non-traversable path"
        );
    }

    #[tokio::test]
    async fn propose_with_unsanctioned_type_is_rejected_and_directed_to_ontology_queue() {
        // ADR-0002: the LLM never invents a type. An unsanctioned type is
        // rejected; the caller is directed to the ontology governance queue.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;

        let err = propose_structural_inference(
            &db,
            maria,
            beta,
            "bamboozles",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(
            err.to_string().contains("/ontology/propose"),
            "directed to the ontology queue: {err:?}"
        );
        assert!(list_inference_proposals(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn propose_with_empty_type_is_bad_request() {
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let err = propose_structural_inference(
            &db,
            maria,
            beta,
            "  ",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_trims_rationale_and_drops_blank() {
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = propose_structural_inference(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
            Some("  a one-line rationale  "),
        )
        .await
        .unwrap();
        assert_eq!(proposal.rationale.as_deref(), Some("a one-line rationale"));

        let blank = propose_structural_inference(
            &db,
            maria,
            beta,
            "helps",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
            Some("   "),
        )
        .await
        .unwrap();
        assert!(blank.rationale.is_none(), "blank rationale dropped");
    }

    // --- endorse / reject ---

    async fn seed_pending(
        db: &Db,
        source: i64,
        target: i64,
        proposed_type: &str,
        path: Vec<EvidenceEdge>,
    ) -> ChatInferenceProposal {
        propose_structural_inference(db, source, target, proposed_type, path, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn endorse_persists_edge_with_structural_inference_provenance_and_type_history() {
        // ADR-0006: on endorsement the edge persists with
        // `asserted_by: [Chat_Inference_ID, mode: structural]`.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;

        let endorsed = endorse_inference_proposal(&db, proposal.id).await.unwrap();
        assert_eq!(endorsed.status, STATUS_ENDORSED);
        assert!(endorsed.resolved_at.is_some());

        // The direct edge Maria —[endangers]→ Beta release now exists.
        let edge = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("endorsed edge persisted");
        assert_eq!(edge.original_type, "endangers");
        // Type history initialised at index 0 (ADR-0003).
        let history = crate::graph::edge_type_history(&db, edge.id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
        // Provenance: this proposal is the asserter, origin structural.
        let assertions = edge_inference_asserted_by(&db, edge.id).await.unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
        // No braindump provenance — the inference is the sole origin.
        assert!(
            crate::graph::edge_provenance(&db, edge.id)
                .await
                .unwrap()
                .is_empty(),
            "structural inference edge has no braindump asserter"
        );
    }

    #[tokio::test]
    async fn endorse_accretes_provenance_when_direct_edge_already_exists() {
        // ADR-0002 accretion: if the direct edge already exists (asserted by
        // a braindump), endorsing adds the inference as a co-asserter rather
        // than duplicating the edge.
        let db = test_db();
        let emb = fake_embedding();

        // Seed the multi-hop path Maria —[endangers]→ Q3 —[depends_on]→ Beta.
        let bd_path = seed_braindump(&db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            &db,
            &emb,
            bd_path,
            "maria endangers q3 which beta depends on",
            extraction(
                &["Maria", "Q3 launch", "Beta release"],
                &[
                    ("Maria", "endangers", "Q3 launch"),
                    ("Q3 launch", "depends_on", "Beta release"),
                ],
            ),
        )
        .await
        .unwrap();
        let maria = concept_id(&db, "Maria").await;
        let q3 = concept_id(&db, "Q3 launch").await;
        let beta = concept_id(&db, "Beta release").await;

        // Separately assert the direct edge Maria —[endangers]→ Beta with a
        // second braindump (Maria and Beta accrete to the existing concepts).
        let bd_direct = seed_braindump(&db, "maria endangers the beta release directly").await;
        ingest_extraction(
            &db,
            &emb,
            bd_direct,
            "maria endangers the beta release directly",
            extraction(
                &["Maria", "Beta release"],
                &[("Maria", "endangers", "Beta release")],
            ),
        )
        .await
        .unwrap();

        let existing_edge = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("direct edge pre-exists");
        assert_eq!(
            crate::graph::edge_provenance(&db, existing_edge.id)
                .await
                .unwrap(),
            vec![bd_direct]
        );

        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        endorse_inference_proposal(&db, proposal.id).await.unwrap();

        // Same edge (no duplicate), now asserted by both the braindump and
        // the structural inference.
        let edge = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("edge still present");
        assert_eq!(edge.id, existing_edge.id, "edge accreted, not duplicated");
        assert_eq!(
            crate::graph::edge_provenance(&db, edge.id).await.unwrap(),
            vec![bd_direct],
            "braindump provenance preserved"
        );
        let assertions = edge_inference_asserted_by(&db, edge.id).await.unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
    }

    #[tokio::test]
    async fn reject_drops_the_proposal_and_persists_no_edge() {
        // ADR-0006: a rejected inference never enters the graph.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;

        let rejected = reject_inference_proposal(&db, proposal.id).await.unwrap();
        assert_eq!(rejected.status, STATUS_REJECTED);
        assert!(rejected.resolved_at.is_some());

        assert!(
            crate::graph::find_edge(&db, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on reject"
        );
        // The rejected proposal stays in the table (audit trail) but is no
        // longer pending.
        let refreshed = get_inference_proposal(&db, proposal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.status, STATUS_REJECTED);
    }

    #[tokio::test]
    async fn endorse_missing_proposal_is_not_found() {
        let db = test_db();
        let err = endorse_inference_proposal(&db, 9999).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_missing_proposal_is_not_found() {
        let db = test_db();
        let err = reject_inference_proposal(&db, 9999).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn endorse_already_endorsed_is_conflict() {
        // Endorsement is immutable — no second chance.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        endorse_inference_proposal(&db, proposal.id).await.unwrap();
        let err = endorse_inference_proposal(&db, proposal.id)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_already_endorsed_is_conflict() {
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        endorse_inference_proposal(&db, proposal.id).await.unwrap();
        let err = reject_inference_proposal(&db, proposal.id)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn inference_backed_edge_survives_braindump_deletion() {
        // ADR-0006: a chat inference is its own provenance origin, distinct
        // from a braindump. An edge backed only by an endorsed inference must
        // survive a braindump deletion — the orphan-edge cascade consults both
        // provenance tables, so deleting the braindump that seeded the path
        // does not orphan the inferred direct edge (even though it has no
        // braindump asserter of its own).
        let db = test_db();
        let emb = fake_embedding();
        let bd = seed_braindump(&db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            &db,
            &emb,
            bd,
            "maria endangers q3 which beta depends on",
            extraction(
                &["Maria", "Q3 launch", "Beta release"],
                &[
                    ("Maria", "endangers", "Q3 launch"),
                    ("Q3 launch", "depends_on", "Beta release"),
                ],
            ),
        )
        .await
        .unwrap();
        let maria = concept_id(&db, "Maria").await;
        let q3 = concept_id(&db, "Q3 launch").await;
        let beta = concept_id(&db, "Beta release").await;

        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        endorse_inference_proposal(&db, proposal.id).await.unwrap();
        let inferred = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("inferred edge persisted");
        // It has only an inference asserter — no braindump provenance.
        assert!(
            crate::graph::edge_provenance(&db, inferred.id)
                .await
                .unwrap()
                .is_empty(),
            "inferred edge has no braindump asserter"
        );

        // Delete the braindump that seeded the path. The path edges vanish
        // (their sole asserter is gone, and the endpoint concepts may vanish
        // too), but the inferred direct edge survives: the inference is its
        // own origin. (If the endpoint concepts vanish, the FK ON DELETE
        // CASCADE on edges would remove it — so this test seeds Maria and
        // Beta only via this one braindump; to keep them alive we add a
        // second extracting braindump for the endpoints.)
        let bd_keep = seed_braindump(&db, "maria and the beta release").await;
        ingest_extraction(
            &db,
            &emb,
            bd_keep,
            "maria and the beta release",
            extraction(&["Maria", "Beta release"], &[]),
        )
        .await
        .unwrap();

        crate::graph::delete_braindump(&db, bd).await.unwrap();

        // The inferred direct edge survives — the inference origin still
        // backs it, and Maria/Beta survive (bd_keep extracts them).
        let survivor = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("inferred edge survives braindump deletion");
        assert_eq!(survivor.id, inferred.id);
        let assertions = edge_inference_asserted_by(&db, survivor.id).await.unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
    }

    #[tokio::test]
    async fn list_returns_proposals_oldest_first() {
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let p1 = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        let p2 = seed_pending(&db, maria, q3, "helps", vec![hop(maria, "endangers", q3)]).await;
        let listed = list_inference_proposals(&db).await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, p1.id);
        assert_eq!(listed[1].id, p2.id);
        assert!(listed.iter().all(|p| p.mode == STRUCTURAL_MODE));
    }

    #[tokio::test]
    async fn structural_proposal_carries_no_thematic_snapshot() {
        // ADR-0009: structural inferences carry NO Thematic Snapshot — their
        // evidence is the graph itself (the `evidence_path`), always present.
        // The proposal row has no snapshot column; the endorse writes none.
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let proposal = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        endorse_inference_proposal(&db, proposal.id).await.unwrap();

        // No snapshot-bearing table is populated by structural mode.
        let snapshot_tables: i64 = db
            .run(|conn| {
                Ok(conn.query_row(
                    "SELECT count(*) FROM sqlite_master
                     WHERE type='table' AND name LIKE '%thematic_snapshot%'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(snapshot_tables, 0, "no snapshot table for structural mode");
        // The assertion carries only the mode — no snapshot reference.
        let edge = crate::graph::find_edge(&db, maria, "endangers", beta)
            .await
            .unwrap()
            .unwrap();
        let assertions = edge_inference_asserted_by(&db, edge.id).await.unwrap();
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
    }
}
