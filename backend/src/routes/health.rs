//! `GET /health` — pings the database and confirms `sqlite-vec` loaded.

use axum::extract::State;
use axum::response::Json;
use serde_json::Value;

use crate::error::Result;
use crate::state::AppState;

pub async fn health(State(state): State<AppState>) -> Result<Json<Value>> {
    let (db_ok, vec_ok) = state
        .db
        .with_conn(|conn| {
            let db_ok: bool = conn.query_row("SELECT 1", [], |_| Ok(true)).is_ok();
            let vec_ok: bool = conn
                .query_row("SELECT vec_length(vec_f32('[1.0,2.0,3.0]'))", [], |_| {
                    Ok(true)
                })
                .is_ok();
            Ok((db_ok, vec_ok))
        })
        .await?;
    Ok(Json(serde_json::json!({
        "ok": db_ok && vec_ok,
        "db": db_ok,
        "sqlite_vec": vec_ok,
    })))
}
