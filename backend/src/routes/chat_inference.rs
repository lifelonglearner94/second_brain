//! Chat write-back routes (issue #11, ADR-0006 structural mode).
//!
//! `POST /chat/inferences` — propose a structural inference: a direct edge
//! summarizing a real, traversable multi-hop edge path. The proposal enters
//! the queue `pending`; it is NEVER auto-endorsed.
//! `GET /chat/inferences` — list the chat-inference proposal queue.
//! `POST /chat/inferences/{id}/endorse` — endorse a pending proposal: persist
//! the edge with `asserted_by: [Chat_Inference_ID, mode: structural]`.
//! `POST /chat/inferences/{id}/reject` — reject a pending proposal: the
//! inference never enters the graph.
//!
//! All four sit behind the auth middleware. The domain logic (path validation,
//! traversability, endorse→persist) lives in [`crate::chat_inference`]; these
//! handlers are the thin HTTP seam.

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

/// `GET /chat/inferences` — the chat-inference proposal queue, oldest first.
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<ChatInferenceProposal>>> {
    let proposals = chat_inference::list_inference_proposals(&state.db).await?;
    Ok(Json(proposals))
}

/// `POST /chat/inferences/{id}/endorse` — endorse a pending proposal: persist
/// the edge with structural-inference provenance.
pub async fn endorse(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ChatInferenceProposal>> {
    let proposal = chat_inference::endorse_inference_proposal(&state.db, id).await?;
    tracing::debug!(proposal_id = id, "structural inference endorsed");
    Ok(Json(proposal))
}

/// `POST /chat/inferences/{id}/reject` — reject a pending proposal: the
/// inference never enters the graph.
pub async fn reject(State(state): State<AppState>, Path(id): Path<i64>) -> Result<StatusCode> {
    chat_inference::reject_inference_proposal(&state.db, id).await?;
    tracing::debug!(proposal_id = id, "structural inference rejected");
    Ok(StatusCode::NO_CONTENT)
}
