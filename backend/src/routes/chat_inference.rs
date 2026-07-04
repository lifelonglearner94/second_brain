//! Chat write-back routes (issues #11 + #13, ADR-0006).
//!
//! `POST /chat/inferences` — propose a structural inference: a direct edge
//! summarizing a real, traversable multi-hop edge path. The proposal enters
//! the queue `pending`; it is NEVER auto-endorsed.
//! `POST /chat/inferences/thematic` — propose a thematic inference: a new
//! edge bridging Louvain cluster-mates with no connecting edge path. Not
//! graph-backed (ADR-0006 thematic mode); carries a frozen Thematic Snapshot
//! (ADR-0009). The proposal enters the queue `pending`; NEVER auto-endorsed.
//! `GET /chat/inferences` — list the chat-inference proposal queue (both modes).
//! `POST /chat/inferences/{id}/endorse` — endorse a pending proposal: persist
//! the edge with `asserted_by: [Chat_Inference_ID, mode: structural|thematic]`.
//! `POST /chat/inferences/{id}/reject` — reject a pending proposal: the
//! inference never enters the graph.
//!
//! All five sit behind the auth middleware. The domain logic (path validation,
//! traversability, snapshot capture, endorse→persist) lives in
//! [`crate::chat_inference`]; these handlers are the thin HTTP seam.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Json;
use serde::Deserialize;

use crate::chat_inference::{self, ChatInferenceProposal, EvidenceEdge};
use crate::error::Result;
use crate::state::AppState;

/// Body for `POST /chat/inferences`. `evidence_path` is the traversable
/// multi-hop edge path that backs the structural inference; the proposed
/// direct edge is `source_concept_id —[proposed_type]→ target_concept_id`.
/// `rationale` is the LLM's optional one-line justification.
#[derive(Debug, Deserialize)]
pub struct ProposeRequest {
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub proposed_type: String,
    pub evidence_path: Vec<EvidenceEdge>,
    pub rationale: Option<String>,
}

/// Body for `POST /chat/inferences/thematic`. `cluster_concept_ids` is the
/// LLM's observation of the motivating cluster's composition (the concepts
/// Louvain grouped together, ADR-0008); the backend computes the snapshot's
/// `braindump_ids` from `edge_provenance` (ADR-0009). The proposed edge is
/// `source_concept_id —[proposed_type]→ target_concept_id`, bridging two
/// cluster-mates with no connecting edge path.
#[derive(Debug, Deserialize)]
pub struct ProposeThematicRequest {
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub proposed_type: String,
    pub cluster_concept_ids: Vec<i64>,
    pub rationale: Option<String>,
}

/// `POST /chat/inferences` — propose a structural inference for human review.
pub async fn propose(
    State(state): State<AppState>,
    Json(body): Json<ProposeRequest>,
) -> Result<Json<ChatInferenceProposal>> {
    let proposal = chat_inference::propose_structural_inference(
        &state.db,
        body.source_concept_id,
        body.target_concept_id,
        &body.proposed_type,
        body.evidence_path,
        body.rationale.as_deref(),
    )
    .await?;
    tracing::debug!(proposal_id = proposal.id, "structural inference proposed");
    Ok(Json(proposal))
}

/// `POST /chat/inferences/thematic` — propose a thematic inference for human
/// review. The proposal carries a frozen Thematic Snapshot (ADR-0009).
pub async fn propose_thematic(
    State(state): State<AppState>,
    Json(body): Json<ProposeThematicRequest>,
) -> Result<Json<ChatInferenceProposal>> {
    let proposal = chat_inference::propose_thematic_inference(
        &state.db,
        body.source_concept_id,
        body.target_concept_id,
        &body.proposed_type,
        body.cluster_concept_ids,
        body.rationale.as_deref(),
    )
    .await?;
    tracing::debug!(proposal_id = proposal.id, "thematic inference proposed");
    Ok(Json(proposal))
}

/// `GET /chat/inferences` — the chat-inference proposal queue (both modes),
/// oldest first.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ChatInferenceProposal>>> {
    let proposals = chat_inference::list_inference_proposals(&state.db).await?;
    Ok(Json(proposals))
}

/// `POST /chat/inferences/{id}/endorse` — endorse a pending proposal: persist
/// the edge with structural- or thematic-inference provenance (per the
/// proposal's mode), plus the Thematic Snapshot for thematic proposals.
pub async fn endorse(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ChatInferenceProposal>> {
    let proposal = chat_inference::endorse_inference_proposal(&state.db, id).await?;
    tracing::debug!(proposal_id = id, mode = %proposal.mode, "inference endorsed");
    Ok(Json(proposal))
}

/// `POST /chat/inferences/{id}/reject` — reject a pending proposal: the
/// inference never enters the graph.
pub async fn reject(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode> {
    chat_inference::reject_inference_proposal(&state.db, id).await?;
    tracing::debug!(proposal_id = id, "inference rejected");
    Ok(StatusCode::NO_CONTENT)
}
