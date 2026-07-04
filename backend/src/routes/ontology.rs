//! `GET /ontology` — the governed edge-type vocabulary, read-only. The LLM
//! draws from this and never invents beyond it; governance (propose/approve,
//! type-embeddings, event-sourced refactor — ADR-0003) is a later slice.

use axum::extract::State;
use axum::response::Json;
use serde::Serialize;

use crate::error::Result;
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
    let edge_types = state
        .db
        .run(|conn| {
            let mut stmt =
                conn.prepare("SELECT slug, label, description FROM ontology ORDER BY id")?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(EdgeType {
                        slug: r.get(0)?,
                        label: r.get(1)?,
                        description: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .await?;
    Ok(Json(Ontology { edge_types }))
}
