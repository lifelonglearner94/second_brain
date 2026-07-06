//! Chat write-back, structural + thematic modes (issues #11 + #13, ADR-0006).
//!
//! Chat is not only a read surface (ADR-0005) but a governed write-back
//! surface. It proposes inferences as candidate edges, routed through the
//! same fractal governance built for concepts (ADR-0001) and types
//! (ADR-0003). Two proposal modes, distinct epistemic status:
//!
//! - **Structural Inference** (issue #11, `origin: structural_inference`): the
//!   LLM traces an existing multi-hop edge path and proposes a direct edge
//!   summarizing it — "the graph supports this; I'm labeling existing
//!   structure." Graph-backed, deterministic, low-risk. The proposal's
//!   evidence is a traversable edge path, captured verbatim in `evidence_path`.
//!
//! - **Thematic Inference** (issue #13, `origin: thematic_inference`): the LLM
//!   observes thematic density in the current Thematic Read Model partition
//!   (ADR-0008) — concepts clustered by Louvain with no connecting edge path —
//!   and proposes a new edge bridging the gap. Not graph-backed: the evidence
//!   is a statistical hypothesis from a non-deterministic partition that won't
//!   exist tomorrow. Riskier by nature.
//!
//! The inference-claim is ALWAYS human-gated — no auto-endorse. Endorsing an
//! LLM deduction is the highest-stakes graph mutation (it can drift the brain
//! toward the LLM's worldview), so the propose→HITL→endorse pattern applies
//! with no tolerance threshold. A proposal starts `pending`; only the explicit
//! endorse endpoint flips it to `endorsed` and persists the edge.
//!
//! On endorsement the edge persists (or accretes, if it already exists) with
//! provenance recorded in `edge_inference_provenance` as
//! `asserted_by: [Chat_Inference_ID, mode: structural|thematic]` — origin-typed
//! so user thoughts (braindump provenance, ADR-0002) and LLM deductions stay
//! distinguishable, and within LLM deductions, graph-backed summaries stay
//! distinguishable from LLM-hallucinated hypotheses. Structural inferences
//! carry NO Thematic Snapshot (ADR-0009): their evidence is the graph itself,
//! always present, recorded in `evidence_path`. Thematic inferences carry a
//! Thematic Snapshot — a frozen capture of the motivating cluster's braindumps
//! — because the ephemeral evidence must be preserved as an audit trail even
//! after the cluster dissolves.
//!
//! After #47 the full propose→HITL→endorse/reject storage flow is behind the
//! [`GraphRepo`] trait. The free functions here are thin delegating wrappers:
//! they own the pure validation (path shape, type non-empty, cluster shape)
//! and delegate the DB work (ontology slug check, traversability, INSERT,
//! edge persistence, status flip) to [`SqliteGraphRepo`]'s trait impl. No
//! `*_conn` helpers live here after #47 — they moved to the adapter.
//!
//! The `InferenceProposer` trait is the natural next step after this slice:
//! splitting Structural and Thematic proposal *generation* (the LLM side —
//! tracing the path, observing the cluster, composing the rationale) behind a
//! separate trait, so the chat route depends on the proposer interface rather
//! than calling the LLM directly. The DB trait (`GraphRepo`) stays pure-DB.

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::{Error, Result};
use crate::graph_repo::{GraphRepo, SqliteGraphRepo};

/// Origin tag for a structural inference proposal (ADR-0006). The
/// graph-backed, deterministic mode — "the graph supports this; I'm labeling
/// existing structure."
pub const STRUCTURAL_MODE: &str = "structural_inference";

/// Origin tag for a thematic inference proposal (ADR-0006). The non-graph-
/// backed, statistical-hypothesis mode — "Louvain clustered these with no
/// connecting edge path; I'm proposing a bridge." Riskier than structural:
/// the evidence is an ephemeral Louvain partition that won't exist tomorrow,
/// so the proposal carries a Thematic Snapshot (ADR-0009) as a frozen receipt.
pub const THEMATIC_MODE: &str = "thematic_inference";

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

/// A frozen capture of a cluster's composition at the moment a Thematic
/// Inference is proposed (ADR-0009). The cluster that motivated the proposal
/// is ephemeral (Louvain is non-deterministic, ADR-0008) and will not exist
/// tomorrow; the snapshot is the historical receipt that lets the user audit,
/// months later, exactly why they endorsed a thematic proposal. Endorsement is
/// immutable — the snapshot is a frozen receipt, never re-evaluated as the
/// partition evolves.
///
/// `braindump_ids` is the braindump evidence whose edges formed the thematic
/// density — computed backend-side from `edge_provenance` (the braindump
/// asserters of edges between cluster concepts) so the receipt is a verifiable
/// computation, not an LLM claim. `concept_ids` is the cluster's concept
/// composition, from the LLM's observation of the current partition (the only
/// thing the backend cannot re-derive — the partition is session-scoped and
/// non-deterministic). Both are frozen at proposal time and survive the
/// deletion of their constituent braindumps/concepts (no FK — ADR-0009).
///
/// Structural inferences carry NO snapshot (ADR-0009): their evidence is the
/// graph itself, always present, recorded in `evidence_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThematicSnapshot {
    pub id: i64,
    pub braindump_ids: Vec<i64>,
    pub concept_ids: Vec<i64>,
    pub captured_at: i64,
}

/// A pending or resolved chat-inference proposal (read model). `status` is
/// `pending`, `endorsed`, or `rejected`. `evidence_path` is the traversable
/// edge path that backs a structural proposal (empty for thematic — not
/// graph-backed). `snapshot` is the frozen Thematic Snapshot carried by a
/// thematic proposal (ADR-0009); `None` for structural proposals.
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
    pub snapshot: Option<ThematicSnapshot>,
}

/// One chat-inference assertion backing an edge (ADR-0006 origin-typed
/// provenance). `mode` is `structural_inference` or `thematic_inference`.
/// `snapshot_id` is the frozen Thematic Snapshot for thematic assertions
/// (ADR-0009); `None` for structural. The braindump-origin half of
/// `asserted_by` lives in `edge_provenance` (ADR-0002); this is the
/// chat-inference half.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InferenceAssertion {
    pub chat_inference_id: i64,
    pub mode: String,
    pub snapshot_id: Option<i64>,
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
///
/// Wrapper: the pure path-shape validation runs here; the DB checks (ontology
/// slug, traversability) and the INSERT are delegated to
/// [`GraphRepo::propose_structural_inference`] (issue #47).
pub async fn propose_structural_inference(
    db: &Db,
    user_id: &str,
    source_concept_id: i64,
    target_concept_id: i64,
    proposed_type: &str,
    evidence_path: Vec<EvidenceEdge>,
    rationale: Option<&str>,
) -> Result<ChatInferenceProposal> {
    if proposed_type.trim().is_empty() {
        return Err(Error::BadRequest("proposed_type must be non-empty".into()));
    }
    validate_path(&evidence_path, source_concept_id, target_concept_id)?;
    SqliteGraphRepo::new(db.clone())
        .propose_structural_inference(
            user_id,
            source_concept_id,
            target_concept_id,
            proposed_type,
            evidence_path,
            rationale,
        )
        .await
}

/// Validate the evidence path is non-empty, connected, and spans
/// `source_concept_id` → `target_concept_id`. Pure (no I/O) so it is
/// hermetically testable. Traversability (each hop is a real edge) is checked
/// against the DB in the trait method.
pub(crate) fn validate_path(
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

/// Propose a thematic inference (ADR-0006 thematic mode): a new edge
/// `source —[proposed_type]→ target` bridging cluster-mates that Louvain
/// grouped together but that have no connecting edge path. Not graph-backed —
/// a statistical hypothesis from a non-deterministic partition that won't
/// exist tomorrow. The proposal enters the queue as `pending` — NEVER
/// auto-endorsed. On a later endorse the edge persists with
/// `asserted_by: [this proposal's id, mode: thematic_inference]` plus the
/// Thematic Snapshot attached to the provenance.
///
/// `cluster_concept_ids` is the LLM's observation of the motivating cluster's
/// composition (the concepts Louvain grouped together). The backend computes
/// the snapshot's `braindump_ids` from `edge_provenance` — the braindumps that
/// asserted edges between cluster concepts — so the frozen receipt is a
/// verifiable computation, not an LLM claim (ADR-0009). The concept_ids come
/// from the LLM because the partition is session-scoped and non-deterministic;
/// the backend cannot re-derive them.
///
/// Validation: `proposed_type` must be a governed ontology slug (ADR-0002);
/// `source` and `target` must be distinct and both in `cluster_concept_ids`
/// (a thematic inference bridges cluster-mates); every cluster concept must
/// exist; the computed `braindump_ids` must be non-empty (a cluster with no
/// braindump-backed edges has no thematic density from user thoughts — a
/// thematic inference must rest on user evidence, not LLM-on-LLM deduction).
/// No graph-traversability check: the partition is non-deterministic, so
/// "no edge path" is the LLM's observation at proposal time; the HITL
/// reviewer is the gate, and the snapshot is the receipt.
///
/// Wrapper: the pure cluster-shape validation runs here; the DB checks
/// (ontology slug, concept existence, braindump computation) and the INSERT
/// are delegated to [`GraphRepo::propose_thematic_inference`] (issue #47).
pub async fn propose_thematic_inference(
    db: &Db,
    user_id: &str,
    source_concept_id: i64,
    target_concept_id: i64,
    proposed_type: &str,
    cluster_concept_ids: Vec<i64>,
    rationale: Option<&str>,
) -> Result<ChatInferenceProposal> {
    if proposed_type.trim().is_empty() {
        return Err(Error::BadRequest("proposed_type must be non-empty".into()));
    }
    if source_concept_id == target_concept_id {
        return Err(Error::BadRequest(
            "source and target must be distinct — a thematic inference bridges two cluster-mates"
                .into(),
        ));
    }
    let mut cluster = cluster_concept_ids.clone();
    cluster.sort_unstable();
    cluster.dedup();
    if cluster.is_empty() {
        return Err(Error::BadRequest(
            "cluster_concept_ids must be non-empty".into(),
        ));
    }
    if !cluster.contains(&source_concept_id) || !cluster.contains(&target_concept_id) {
        return Err(Error::BadRequest(
            "source_concept_id and target_concept_id must be in cluster_concept_ids — \
             a thematic inference bridges cluster-mates (ADR-0006)"
                .into(),
        ));
    }
    SqliteGraphRepo::new(db.clone())
        .propose_thematic_inference(
            user_id,
            source_concept_id,
            target_concept_id,
            proposed_type,
            cluster_concept_ids,
            rationale,
        )
        .await
}

/// Endorse a pending chat-inference proposal (ADR-0006): persist the
/// direct edge `source —[proposed_type]→ target` and record the proposal as
/// its chat-inference asserter. The edge accretes (ADR-0002): if the direct
/// edge already exists — asserted by a braindump, or by an earlier endorsed
/// inference — the new assertion is added to its provenance rather than
/// duplicated. Type history is initialised at index 0 for a newly-created
/// edge (ADR-0003). The proposal's `mode` origin-tags the persisted
/// provenance (`structural_inference` or `thematic_inference`); for a
/// thematic proposal, the frozen Thematic Snapshot (ADR-0009) is attached
/// to the provenance row so the user can audit the ephemeral cluster that
/// motivated the edge, even after the cluster dissolves.
///
/// `NotFound` if the proposal does not exist; `Conflict` if it is not
/// `pending` (already endorsed or rejected — endorsement is immutable, no
/// second chance). Returns the refreshed proposal.
///
/// Wrapper: delegates to [`GraphRepo::endorse_inference_proposal`] (issue #47).
pub async fn endorse_inference_proposal(
    db: &Db,
    user_id: &str,
    id: i64,
) -> Result<ChatInferenceProposal> {
    SqliteGraphRepo::new(db.clone())
        .endorse_inference_proposal(user_id, id)
        .await
}

/// Reject a pending structural inference proposal (ADR-0006): keep the graph
/// untouched and mark the proposal `rejected`. No edge is persisted — the
/// inference-claim never enters the graph. `NotFound` if the proposal does
/// not exist; `Conflict` if it is not `pending`. Returns the refreshed
/// proposal.
///
/// Wrapper: delegates to [`GraphRepo::reject_inference_proposal`] (issue #47).
pub async fn reject_inference_proposal(
    db: &Db,
    user_id: &str,
    id: i64,
) -> Result<ChatInferenceProposal> {
    SqliteGraphRepo::new(db.clone())
        .reject_inference_proposal(user_id, id)
        .await
}

/// List all chat-inference proposals, oldest first.
///
/// Wrapper: delegates to [`GraphRepo::list_inference_proposals`] (issue #47).
pub async fn list_inference_proposals(
    db: &Db,
    user_id: &str,
) -> Result<Vec<ChatInferenceProposal>> {
    SqliteGraphRepo::new(db.clone())
        .list_inference_proposals(user_id)
        .await
}

/// Look up a single proposal by id. `None` if no row matches.
///
/// Wrapper: delegates to [`GraphRepo::get_inference_proposal`] (issue #47).
pub async fn get_inference_proposal(
    db: &Db,
    user_id: &str,
    id: i64,
) -> Result<Option<ChatInferenceProposal>> {
    SqliteGraphRepo::new(db.clone())
        .get_inference_proposal(user_id, id)
        .await
}

/// The chat-inference assertions backing an edge (ADR-0006 origin-typed
/// provenance). The braindump-origin half of `asserted_by` lives in
/// `edge_provenance` (ADR-0002); this is the chat-inference half. An edge's
/// full asserter list is the union of both. `snapshot_id` is the frozen
/// Thematic Snapshot for thematic assertions (ADR-0009); `None` for structural.
///
/// Wrapper: delegates to [`GraphRepo::edge_inference_asserted_by`] (issue #47).
pub async fn edge_inference_asserted_by(
    db: &Db,
    user_id: &str,
    edge_id: i64,
) -> Result<Vec<InferenceAssertion>> {
    SqliteGraphRepo::new(db.clone())
        .edge_inference_asserted_by(user_id, edge_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;
    use crate::error::Error;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::ingest_extraction;
    use crate::llm::{FakeLlm, Llm};

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeLlm::default().dim()).unwrap();
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

    async fn seed_braindump(db: &Db, text: &str) -> i64 {
        let b = insert_braindump(db, BOOTSTRAP_ADMIN_USER_ID, text, text)
            .await
            .unwrap();
        b.id
    }

    async fn concept_id(db: &Db, label: &str) -> i64 {
        crate::graph::concept_id_for_label(db, BOOTSTRAP_ADMIN_USER_ID, label)
            .await
            .unwrap()
            .unwrap()
    }

    /// Seed the canonical ADR-0006 structural path:
    /// `Maria —[endangers]→ Q3 launch —[depends_on]→ Beta release`.
    /// Returns the three concept ids in that order.
    async fn seed_path(db: &Db) -> (i64, i64, i64) {
        let llm = fake_llm();
        let bd = seed_braindump(db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
            BOOTSTRAP_ADMIN_USER_ID,
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
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on a pending proposal (no auto-endorse)"
        );
        assert!(
            edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, 9999)
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
            BOOTSTRAP_ADMIN_USER_ID,
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
            list_inference_proposals(&db, BOOTSTRAP_ADMIN_USER_ID)
                .await
                .unwrap()
                .is_empty(),
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
            BOOTSTRAP_ADMIN_USER_ID,
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
        assert!(list_inference_proposals(&db, BOOTSTRAP_ADMIN_USER_ID)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn propose_with_empty_type_is_bad_request() {
        let db = test_db();
        let (maria, q3, beta) = seed_path(&db).await;
        let err = propose_structural_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
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
            BOOTSTRAP_ADMIN_USER_ID,
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
            BOOTSTRAP_ADMIN_USER_ID,
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
        propose_structural_inference(
            db,
            BOOTSTRAP_ADMIN_USER_ID,
            source,
            target,
            proposed_type,
            path,
            None,
        )
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

        let endorsed = endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        assert_eq!(endorsed.status, STATUS_ENDORSED);
        assert!(endorsed.resolved_at.is_some());

        // The direct edge Maria —[endangers]→ Beta release now exists.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("endorsed edge persisted");
        assert_eq!(edge.original_type, "endangers");
        // Type history initialised at index 0 (ADR-0003).
        let history = crate::graph::edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
        // Provenance: this proposal is the asserter, origin structural.
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
        // No braindump provenance — the inference is the sole origin.
        assert!(
            crate::graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
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
        let llm = fake_llm();

        // Seed the multi-hop path Maria —[endangers]→ Q3 —[depends_on]→ Beta.
        let bd_path = seed_braindump(&db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_direct,
            "maria endangers the beta release directly",
            extraction(
                &["Maria", "Beta release"],
                &[("Maria", "endangers", "Beta release")],
            ),
        )
        .await
        .unwrap();

        let existing_edge =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .expect("direct edge pre-exists");
        assert_eq!(
            crate::graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, existing_edge.id)
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
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();

        // Same edge (no duplicate), now asserted by both the braindump and
        // the structural inference.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("edge still present");
        assert_eq!(edge.id, existing_edge.id, "edge accreted, not duplicated");
        assert_eq!(
            crate::graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap(),
            vec![bd_direct],
            "braindump provenance preserved"
        );
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
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

        let rejected = reject_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        assert_eq!(rejected.status, STATUS_REJECTED);
        assert!(rejected.resolved_at.is_some());

        assert!(
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on reject"
        );
        // The rejected proposal stays in the table (audit trail) but is no
        // longer pending.
        let refreshed = get_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.status, STATUS_REJECTED);
    }

    #[tokio::test]
    async fn endorse_missing_proposal_is_not_found() {
        let db = test_db();
        let err = endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, 9999)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_missing_proposal_is_not_found() {
        let db = test_db();
        let err = reject_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, 9999)
            .await
            .unwrap_err();
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
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        let err = endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
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
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        let err = reject_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
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
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        let inferred =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .expect("inferred edge persisted");
        // It has only an inference asserter — no braindump provenance.
        assert!(
            crate::graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, inferred.id)
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
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_keep,
            "maria and the beta release",
            extraction(&["Maria", "Beta release"], &[]),
        )
        .await
        .unwrap();

        crate::graph::delete_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, bd)
            .await
            .unwrap();

        // The inferred direct edge survives — the inference origin still
        // backs it, and Maria/Beta survive (bd_keep extracts them).
        let survivor =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .expect("inferred edge survives braindump deletion");
        assert_eq!(survivor.id, inferred.id);
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, survivor.id)
            .await
            .unwrap();
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
        let listed = list_inference_proposals(&db, BOOTSTRAP_ADMIN_USER_ID)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, p1.id);
        assert_eq!(listed[1].id, p2.id);
        assert!(listed.iter().all(|p| p.mode == STRUCTURAL_MODE));
    }

    #[tokio::test]
    async fn structural_proposal_carries_no_thematic_snapshot() {
        // ADR-0009: structural inferences carry NO Thematic Snapshot — their
        // evidence is the graph itself (the `evidence_path`), always present.
        // The `thematic_snapshots` table exists (thematic mode uses it, issue
        // #13) but structural mode never populates it; the proposal's
        // `snapshot_id` is NULL and the endorse writes NULL to provenance.
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
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();

        // Structural mode writes no snapshot rows.
        let snapshot_rows: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT count(*) FROM thematic_snapshots", [], |r| r.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(snapshot_rows, 0, "structural mode writes no snapshot");
        // The proposal's snapshot is None.
        assert!(
            proposal.snapshot.is_none(),
            "structural proposal has no snapshot"
        );
        // The provenance assertion carries the mode but no snapshot.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .unwrap();
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(assertions[0].mode, STRUCTURAL_MODE);
        assert!(
            assertions[0].snapshot_id.is_none(),
            "structural provenance has no snapshot"
        );
    }

    // --- issue #13: thematic inference (ADR-0006 thematic mode + ADR-0009) ---

    /// Seed the canonical thematic cluster: `Maria —[endangers]→ Q3 launch
    /// —[depends_on]→ Beta release`, all extracted from one braindump. Louvain
    /// sees a single connected component; the braindump that asserted the edges
    /// is the snapshot evidence. There is no direct Maria→Beta edge — the
    /// thematic gap the LLM bridges. Returns (maria, q3, beta, cluster_bd).
    async fn seed_cluster(db: &Db) -> (i64, i64, i64, i64) {
        let (maria, q3, beta) = seed_path(db).await;
        let bd: i64 = db
            .with_conn(move |conn| {
                Ok(conn.query_row(
                    "SELECT ep.braindump_id
                     FROM edge_provenance ep
                     JOIN edges e ON ep.edge_id = e.id
                     WHERE e.source_concept_id = ?1 AND e.target_concept_id = ?2
                     ORDER BY ep.braindump_id LIMIT 1",
                    rusqlite::params![maria, q3],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        (maria, q3, beta, bd)
    }

    #[tokio::test]
    async fn propose_thematic_inference_creates_pending_proposal_with_frozen_snapshot() {
        // ADR-0006 thematic mode + ADR-0009: the LLM observes a Louvain cluster
        // (Maria/Q3/Beta — connected by edges but no direct Maria→Beta edge)
        // and proposes a bridging edge Maria —[endangers]→ Beta. The proposal
        // carries a Thematic Snapshot: the braindump ids whose edges formed
        // the thematic density, frozen at proposal time. The proposal is
        // pending — no auto-endorse.
        let db = test_db();
        let (maria, q3, beta, cluster_bd) = seed_cluster(&db).await;

        let proposal = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "endangers",
            vec![maria, q3, beta],
            Some("  Louvain clustered Maria/Q3/Beta with no direct Maria→Beta edge  "),
        )
        .await
        .unwrap();

        assert_eq!(proposal.mode, THEMATIC_MODE);
        assert_eq!(proposal.status, STATUS_PENDING);
        assert_eq!(proposal.source_concept_id, maria);
        assert_eq!(proposal.target_concept_id, beta);
        assert_eq!(proposal.proposed_type, "endangers");
        assert!(
            proposal.evidence_path.is_empty(),
            "thematic mode has no evidence path — not graph-backed"
        );
        assert_eq!(
            proposal.rationale.as_deref(),
            Some("Louvain clustered Maria/Q3/Beta with no direct Maria→Beta edge"),
            "rationale trimmed"
        );
        assert!(proposal.resolved_at.is_none());

        // The snapshot is the frozen receipt: the braindump ids whose edges
        // formed the density, plus the cluster's concept composition.
        let snapshot = proposal
            .snapshot
            .as_ref()
            .expect("thematic proposal carries a Thematic Snapshot");
        assert_eq!(
            snapshot.braindump_ids,
            vec![cluster_bd],
            "snapshot captured the cluster's braindump evidence (computed backend-side)"
        );
        assert_eq!(
            snapshot.concept_ids,
            vec![maria, q3, beta],
            "snapshot captured the cluster's composition (LLM's observation)"
        );
        // No edge persisted yet — no auto-endorse.
        assert!(
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on a pending thematic proposal"
        );
        // The snapshot row is in the table (frozen receipt).
        let snapshot_rows: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT count(*) FROM thematic_snapshots", [], |r| r.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(snapshot_rows, 1, "one snapshot row per proposal");
    }

    /// Propose a pending thematic inference (helper for the endorse tests).
    async fn seed_pending_thematic(
        db: &Db,
        source: i64,
        target: i64,
        proposed_type: &str,
        cluster: Vec<i64>,
    ) -> ChatInferenceProposal {
        propose_thematic_inference(
            db,
            BOOTSTRAP_ADMIN_USER_ID,
            source,
            target,
            proposed_type,
            cluster,
            None,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn endorse_thematic_persists_edge_with_thematic_provenance_and_snapshot() {
        // ADR-0006 + ADR-0009: on endorsement the edge persists with
        // `asserted_by: [Chat_Inference_ID, mode: thematic_inference]` and
        // the frozen Thematic Snapshot attached to the provenance row. The
        // snapshot_id on edge_inference_provenance must match the proposal's
        // snapshot — the receipt travels with the edge so the user can audit
        // the ephemeral cluster months later.
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let proposal =
            seed_pending_thematic(&db, maria, beta, "endangers", vec![maria, q3, beta]).await;
        let snapshot_id = proposal.snapshot.as_ref().expect("snapshot present").id;

        let endorsed = endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        assert_eq!(endorsed.status, STATUS_ENDORSED);
        assert!(endorsed.resolved_at.is_some());

        // The direct edge Maria —[endangers]→ Beta persists.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("endorsed thematic edge persisted");
        assert_eq!(edge.original_type, "endangers");
        // Type history initialised at index 0 (ADR-0003).
        let history = crate::graph::edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].seq_index, 0);
        assert_eq!(history[0].type_slug, "endangers");
        // Provenance: this proposal is the asserter, origin thematic, with snapshot.
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, THEMATIC_MODE);
        assert_eq!(
            assertions[0].snapshot_id,
            Some(snapshot_id),
            "the frozen snapshot is attached to the persisted provenance"
        );
        // No braindump provenance — the inference is the sole origin.
        assert!(
            crate::graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap()
                .is_empty(),
            "thematic inference edge has no braindump asserter"
        );
        // The snapshot row is unchanged (frozen receipt, not re-evaluated).
        let refreshed = get_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            refreshed.snapshot.as_ref().unwrap().id,
            snapshot_id,
            "snapshot id stable across endorse"
        );
    }

    #[tokio::test]
    async fn endorse_thematic_accretes_onto_existing_edge_with_snapshot() {
        // ADR-0002 accretion: if the direct edge already exists (asserted by
        // a braindump), endorsing a thematic inference adds the inference as
        // a co-asserter with its snapshot, rather than duplicating the edge.
        let db = test_db();
        let llm = fake_llm();
        // Seed the cluster path Maria —[endangers]→ Q3 —[depends_on]→ Beta.
        let bd_path = seed_braindump(&db, "maria endangers q3 which beta depends on").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
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
        // Separately assert the direct edge Maria —[endangers]→ Beta.
        let bd_direct = seed_braindump(&db, "maria endangers the beta release directly").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_direct,
            "maria endangers the beta release directly",
            extraction(
                &["Maria", "Beta release"],
                &[("Maria", "endangers", "Beta release")],
            ),
        )
        .await
        .unwrap();
        let existing_edge =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .expect("direct edge pre-exists");

        let proposal =
            seed_pending_thematic(&db, maria, beta, "endangers", vec![maria, q3, beta]).await;
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();

        // Same edge (no duplicate), now asserted by both the braindump and
        // the thematic inference with its snapshot.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .expect("edge still present");
        assert_eq!(edge.id, existing_edge.id, "edge accreted, not duplicated");
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(assertions.len(), 1);
        assert_eq!(assertions[0].chat_inference_id, proposal.id);
        assert_eq!(assertions[0].mode, THEMATIC_MODE);
        assert!(
            assertions[0].snapshot_id.is_some(),
            "snapshot attached on accretion"
        );
    }

    #[tokio::test]
    async fn thematic_snapshot_is_immutable_across_partition_evolution() {
        // ADR-0009: endorsement is immutable; the snapshot is a frozen receipt,
        // never re-evaluated as the partition evolves. After endorsing a
        // thematic proposal, adding more braindumps (which changes the graph
        // topology and would produce a different Louvain partition) must NOT
        // change the snapshot's braindump_ids or concept_ids — the receipt is
        // frozen at proposal time.
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let proposal =
            seed_pending_thematic(&db, maria, beta, "endangers", vec![maria, q3, beta]).await;
        let snapshot_before = proposal.snapshot.clone().unwrap();
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();

        // Add a new braindump that changes the graph topology (a new concept
        // + edge). The partition that motivated the proposal is now stale.
        let llm = fake_llm();
        let bd_new = seed_braindump(&db, "gamma produces delta").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_new,
            "gamma produces delta",
            extraction(&["Gamma", "Delta"], &[("Gamma", "produces", "Delta")]),
        )
        .await
        .unwrap();

        // The endorsed proposal's snapshot is unchanged — frozen receipt.
        let refreshed = get_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap()
            .unwrap();
        let snapshot_after = refreshed.snapshot.as_ref().unwrap();
        assert_eq!(snapshot_after.id, snapshot_before.id, "snapshot id stable");
        assert_eq!(
            snapshot_after.braindump_ids, snapshot_before.braindump_ids,
            "braindump_ids frozen — not recomputed"
        );
        assert_eq!(
            snapshot_after.concept_ids, snapshot_before.concept_ids,
            "concept_ids frozen — not recomputed"
        );
        // The edge's provenance snapshot_id is also stable.
        let edge = crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
            .await
            .unwrap()
            .unwrap();
        let assertions = edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
            .await
            .unwrap();
        assert_eq!(assertions[0].snapshot_id, Some(snapshot_before.id));
    }

    #[tokio::test]
    async fn thematic_and_structural_inferences_are_distinguishable_by_origin_tag() {
        // ADR-0006: the explicit origin tag lets the HITL queue distinguish
        // graph-backed proposals from LLM-hallucinated hypotheses. A thematic
        // and a structural proposal for the same edge carry different modes;
        // after both are endorsed, the edge's provenance lists both origins.
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;

        // Structural: the real path Maria —[endangers]→ Q3 —[depends_on]→ Beta.
        let structural = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        // Thematic: a Louvain-motivated bridge between the same pair.
        let thematic =
            seed_pending_thematic(&db, maria, beta, "helps", vec![maria, q3, beta]).await;

        assert_eq!(structural.mode, STRUCTURAL_MODE);
        assert_eq!(thematic.mode, THEMATIC_MODE);
        assert!(
            structural.snapshot.is_none(),
            "structural carries no snapshot"
        );
        assert!(thematic.snapshot.is_some(), "thematic carries a snapshot");

        // Endorse both — they persist as different typed edges (endangers vs
        // helps) with distinguishable provenance origins.
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, structural.id)
            .await
            .unwrap();
        endorse_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, thematic.id)
            .await
            .unwrap();

        let endanger_edge =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .unwrap();
        let helps_edge =
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "helps", beta)
                .await
                .unwrap()
                .unwrap();
        let endanger_assertions =
            edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, endanger_edge.id)
                .await
                .unwrap();
        let helps_assertions =
            edge_inference_asserted_by(&db, BOOTSTRAP_ADMIN_USER_ID, helps_edge.id)
                .await
                .unwrap();
        assert_eq!(endanger_assertions[0].mode, STRUCTURAL_MODE);
        assert!(endanger_assertions[0].snapshot_id.is_none());
        assert_eq!(helps_assertions[0].mode, THEMATIC_MODE);
        assert!(helps_assertions[0].snapshot_id.is_some());
    }

    #[tokio::test]
    async fn propose_thematic_rejects_self_edge() {
        let db = test_db();
        let (maria, q3, _beta, _bd) = seed_cluster(&db).await;
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            maria,
            "endangers",
            vec![maria, q3],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(err.to_string().contains("distinct"), "{err:?}");
    }

    #[tokio::test]
    async fn propose_thematic_rejects_endpoints_not_in_cluster() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        // Maria is in the cluster but the cluster list omits Beta — a
        // thematic inference must bridge cluster-mates.
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "endangers",
            vec![maria, q3],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(err.to_string().contains("cluster-mates"), "{err:?}");
    }

    #[tokio::test]
    async fn propose_thematic_rejects_unsanctioned_type() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "bamboozles",
            vec![maria, q3, beta],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(err.to_string().contains("/ontology/propose"), "{err:?}");
    }

    #[tokio::test]
    async fn propose_thematic_rejects_nonexistent_cluster_concept() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let ghost = 9999;
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "endangers",
            vec![maria, q3, beta, ghost],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(err.to_string().contains("does not exist"), "{err:?}");
    }

    #[tokio::test]
    async fn propose_thematic_rejects_cluster_with_no_braindump_evidence() {
        // A cluster of concepts with no braindump-backed edges between them
        // has no thematic density from user thoughts. A thematic inference
        // must rest on user evidence, not LLM-on-LLM deduction.
        let db = test_db();
        let llm = fake_llm();
        // Two concepts extracted from separate braindumps, no edges between them.
        let bd1 = seed_braindump(&db, "thinking about alpha").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd1,
            "thinking about alpha",
            extraction(&["Alpha"], &[]),
        )
        .await
        .unwrap();
        let bd2 = seed_braindump(&db, "thinking about beta").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd2,
            "thinking about beta",
            extraction(&["Beta"], &[]),
        )
        .await
        .unwrap();
        let alpha = concept_id(&db, "Alpha").await;
        let beta = concept_id(&db, "Beta").await;

        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            alpha,
            beta,
            "endangers",
            vec![alpha, beta],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
        assert!(
            err.to_string().contains("no braindump-backed edges"),
            "{err:?}"
        );
        // No snapshot or proposal created.
        let snapshot_rows: i64 = db
            .with_conn(|conn| {
                Ok(conn.query_row("SELECT count(*) FROM thematic_snapshots", [], |r| r.get(0))?)
            })
            .await
            .unwrap();
        assert_eq!(snapshot_rows, 0, "no snapshot written on rejection");
        assert!(list_inference_proposals(&db, BOOTSTRAP_ADMIN_USER_ID)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn propose_thematic_rejects_empty_type() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "  ",
            vec![maria, q3, beta],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_thematic_rejects_empty_cluster() {
        let db = test_db();
        let (maria, _q3, beta, _bd) = seed_cluster(&db).await;
        let err = propose_thematic_inference(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            maria,
            beta,
            "endangers",
            vec![],
            None,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn list_returns_thematic_and_structural_proposals_oldest_first() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let p1 = seed_pending(
            &db,
            maria,
            beta,
            "endangers",
            vec![hop(maria, "endangers", q3), hop(q3, "depends_on", beta)],
        )
        .await;
        let p2 = seed_pending_thematic(&db, maria, beta, "helps", vec![maria, q3, beta]).await;
        let listed = list_inference_proposals(&db, BOOTSTRAP_ADMIN_USER_ID)
            .await
            .unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, p1.id);
        assert_eq!(listed[1].id, p2.id);
        assert_eq!(listed[0].mode, STRUCTURAL_MODE);
        assert_eq!(listed[1].mode, THEMATIC_MODE);
        assert!(listed[0].snapshot.is_none());
        assert!(listed[1].snapshot.is_some());
    }

    #[tokio::test]
    async fn reject_thematic_keeps_graph_untouched_and_marks_rejected() {
        let db = test_db();
        let (maria, q3, beta, _bd) = seed_cluster(&db).await;
        let proposal =
            seed_pending_thematic(&db, maria, beta, "endangers", vec![maria, q3, beta]).await;

        let rejected = reject_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap();
        assert_eq!(rejected.status, STATUS_REJECTED);
        assert!(rejected.resolved_at.is_some());
        assert!(
            crate::graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", beta)
                .await
                .unwrap()
                .is_none(),
            "no edge persisted on reject"
        );
        // The snapshot row remains (audit trail of what was proposed) but the
        // proposal is no longer pending.
        let refreshed = get_inference_proposal(&db, BOOTSTRAP_ADMIN_USER_ID, proposal.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed.status, STATUS_REJECTED);
        assert!(refreshed.snapshot.is_some(), "snapshot preserved on reject");
    }
}
