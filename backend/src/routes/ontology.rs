//! Ontology routes (issues #3, #9, ADR-0003).
//!
//! `GET /ontology` — the governed edge-type vocabulary, read-only. The LLM
//! draws from this and never invents beyond it.
//!
//! `POST /ontology/propose` — propose a new edge type; embedding-deduped
//! (>99.5% auto-merge, else queued).
//! `GET /ontology/proposals` — list the proposal queue.
//! `POST /ontology/proposals/{id}/approve` — approve a pending proposal: adds
//! the type to the ontology and spawns the async refactor (if `merge_of`).
//! `POST /ontology/proposals/{id}/reject` — reject a pending proposal.

use axum::extract::{Path, State};
use axum::response::Json;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::ontology::{
    approve_proposal, list_proposals, propose_type, reject_proposal, TypeProposal,
};
use crate::state::AppState;

#[derive(Serialize)]
pub struct EdgeType {
    pub slug: String,
    pub label: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct Ontology {
    pub edge_types: Vec<EdgeType>,
}

pub async fn ontology(State(state): State<AppState>) -> Result<Json<Ontology>> {
    // The duplicated full-row ontology query lives in exactly one place now
    // (issue #45): the Sqlite adapter's `GraphRepo::ontology_types` impl.
    let rows = state.graph_repo.ontology_types().await?;
    let edge_types = rows
        .into_iter()
        .map(|(slug, label, description)| EdgeType {
            slug,
            label,
            description,
        })
        .collect();
    Ok(Json(Ontology { edge_types }))
}

/// Body for `POST /ontology/propose`. `merge_of` is the slug of the existing
/// type this proposal merges into the new one — on approve, the refactor
/// retags edges of that type. `None` for a pure new type (no refactor).
#[derive(Debug, Deserialize)]
pub struct ProposeRequest {
    pub slug: String,
    pub label: String,
    pub description: String,
    pub merge_of: Option<String>,
}

#[derive(Serialize)]
pub struct ProposeResponse {
    pub id: i64,
    pub slug: String,
    pub label: String,
    pub description: String,
    pub merge_of: Option<String>,
    pub status: String,
    pub near_match_slug: Option<String>,
    pub near_match_similarity: Option<f32>,
}

impl From<TypeProposal> for ProposeResponse {
    fn from(p: TypeProposal) -> Self {
        Self {
            id: p.id,
            slug: p.slug,
            label: p.label,
            description: p.description,
            merge_of: p.merge_of,
            status: p.status,
            near_match_slug: p.near_match_slug,
            near_match_similarity: p.near_match_similarity,
        }
    }
}

pub async fn propose(
    State(state): State<AppState>,
    Json(body): Json<ProposeRequest>,
) -> Result<Json<ProposeResponse>> {
    let outcome = propose_type(
        &state.db,
        state.llm.as_ref(),
        &body.slug,
        &body.label,
        &body.description,
        body.merge_of.as_deref(),
    )
    .await?;
    Ok(Json(outcome.proposal.into()))
}

#[derive(Serialize)]
pub struct ProposalsResponse {
    pub proposals: Vec<ProposeResponse>,
}

pub async fn proposals(State(state): State<AppState>) -> Result<Json<ProposalsResponse>> {
    let rows = list_proposals(&state.db).await?;
    Ok(Json(ProposalsResponse {
        proposals: rows.into_iter().map(Into::into).collect(),
    }))
}

pub async fn approve(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ProposeResponse>> {
    let proposal = approve_proposal(&state.db, state.llm.as_ref(), id).await?;
    // If the approved type merges another, spawn the refactor out-of-band so
    // ingest is not blocked while it runs (ADR-0003). Fire-and-forget; the
    // JoinHandle is tracked on the runner so tests can await it.
    state
        .refactor_runner
        .spawn(state.db.clone(), state.llm.clone(), proposal.clone());
    Ok(Json(proposal.into()))
}

pub async fn reject(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Json<ProposeResponse>> {
    let proposal = reject_proposal(&state.db, id).await?;
    Ok(Json(proposal.into()))
}
