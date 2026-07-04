//! `POST /chat` — the chat read surface (issue #10, ADR-0005).
//!
//! Runs the retrieval read path (ADR-0004), then synthesizes over the
//! retrieved braindumps + traversed edge paths under a grounded-synthesis
//! system prompt. Every claim cites braindump ids + edge refs; inference weaves
//! only along edges that actually exist; when the graph doesn't support an
//! answer, chat is silent. The full synthesis + silence logic lives in
//! [`crate::chat`]; this handler is the thin HTTP seam.
//!
//! Sits behind the auth middleware (registered in [`crate::routes`] under the
//! protected layer), like the retrieval read path.

use axum::extract::State;
use axum::response::Json;
use serde::Deserialize;

use crate::chat::{self, ChatResponse};
use crate::error::{Error, Result};
use crate::state::AppState;

/// Body for `POST /chat`: the query text. Empty queries are rejected — a chat
/// with no query is a no-op.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub query: String,
}

/// `POST /chat` — run retrieval + grounded synthesis (or return silence) for
/// the query and return the answer with its citations.
pub async fn chat(
    State(state): State<AppState>,
    Json(body): Json<ChatRequest>,
) -> Result<Json<ChatResponse>> {
    let query = body.query;
    if query.trim().is_empty() {
        return Err(Error::BadRequest("query must be non-empty".into()));
    }
    let response = chat::chat(
        &state.db,
        state.embedding.as_ref(),
        state.llm.as_ref(),
        &query,
    )
    .await?;
    Ok(Json(response))
}
