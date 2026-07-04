//! Braindump persistence (issue #5 / ADR-0007).
//!
//! A braindump is an immutable thought-snapshot: verbatim (user-confirmed
//! text at submit), cleaned (LLM-produced rendering), and a timestamp. Edits
//! are error-correction only — they overwrite the verbatim in place and
//! re-clean; substantive thinking-evolution spawns a new braindump, never
//! edits the old one. Extraction is stubbed in this slice (the extractor seam
//! lives in [`crate::extractor`]); this module is pure persistence.

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::db::{now_seconds, Db};
use crate::error::Result;

/// One immutable thought-snapshot. `verbatim` is the user-confirmed text at
/// submit (overwritable only via the edit/error-correction path); `cleaned`
/// is the LLM-produced rendering shown by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Braindump {
    pub id: i64,
    pub verbatim: String,
    pub cleaned: String,
    pub created_at: i64,
}

/// Persist a new braindump with its verbatim and cleaned rendering. Returns
/// the row as stored, with the surrogate id and created_at filled in.
pub async fn insert_braindump(db: &Db, verbatim: &str, cleaned: &str) -> Result<Braindump> {
    let verbatim = verbatim.to_string();
    let cleaned = cleaned.to_string();
    db.run(move |conn| {
        let created_at = now_seconds();
        conn.execute(
            "INSERT INTO braindumps (verbatim, cleaned, created_at)
             VALUES (?1, ?2, ?3)",
            params![verbatim, cleaned, created_at],
        )?;
        let id = conn.last_insert_rowid();
        Ok(Braindump {
            id,
            verbatim,
            cleaned,
            created_at,
        })
    })
    .await
}

/// Look up a braindump by id. `None` if no row matches.
pub async fn get_braindump(db: &Db, id: i64) -> Result<Option<Braindump>> {
    db.run(move |conn| {
        let row = conn
            .query_row(
                "SELECT id, verbatim, cleaned, created_at
                 FROM braindumps WHERE id = ?1",
                params![id],
                |row| {
                    Ok(Braindump {
                        id: row.get(0)?,
                        verbatim: row.get(1)?,
                        cleaned: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    })
    .await
}

/// Overwrite the verbatim in place (error-correction, ADR-0007) and store the
/// re-cleaned rendering. The id and created_at are untouched — a braindump's
/// timestamp is its original submit instant, not its last edit. Returns the
/// updated row, or `None` if no braindump with `id` exists.
pub async fn overwrite_verbatim(
    db: &Db,
    id: i64,
    verbatim: &str,
    cleaned: &str,
) -> Result<Option<Braindump>> {
    let verbatim = verbatim.to_string();
    let cleaned = cleaned.to_string();
    db.run(move |conn| {
        let updated = conn.execute(
            "UPDATE braindumps SET verbatim = ?1, cleaned = ?2 WHERE id = ?3",
            params![verbatim, cleaned, id],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        let row = conn.query_row(
            "SELECT id, verbatim, cleaned, created_at
                 FROM braindumps WHERE id = ?1",
            params![id],
            |row| {
                Ok(Braindump {
                    id: row.get(0)?,
                    verbatim: row.get(1)?,
                    cleaned: row.get(2)?,
                    created_at: row.get(3)?,
                })
            },
        )?;
        Ok(Some(row))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn insert_then_get_returns_stored_braindump() {
        let db = Db::open_in_memory().unwrap();
        let inserted = insert_braindump(&db, "hello world", "Hello, world.")
            .await
            .unwrap();
        let fetched = get_braindump(&db, inserted.id).await.unwrap().unwrap();
        assert_eq!(fetched, inserted);
        assert!(fetched.created_at > 0);
    }

    #[tokio::test]
    async fn get_missing_braindump_is_none() {
        let db = Db::open_in_memory().unwrap();
        let got = get_braindump(&db, 9999).await.unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn overwrite_verbatim_replaces_in_place_keeping_id_and_timestamp() {
        let db = Db::open_in_memory().unwrap();
        let original = insert_braindump(&db, "hallo welt", "Hallo, Welt.")
            .await
            .unwrap();
        let updated = overwrite_verbatim(&db, original.id, "hello world", "Hello, world.")
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(updated.id, original.id, "id is stable across edit");
        assert_eq!(
            updated.created_at, original.created_at,
            "timestamp is stable across edit"
        );
        assert_eq!(updated.verbatim, "hello world");
        assert_eq!(updated.cleaned, "Hello, world.");
        let refetched = get_braindump(&db, original.id).await.unwrap().unwrap();
        assert_eq!(refetched, updated);
    }

    #[tokio::test]
    async fn overwrite_verbatim_on_missing_id_is_none() {
        let db = Db::open_in_memory().unwrap();
        let got = overwrite_verbatim(&db, 9999, "x", "X").await.unwrap();
        assert!(got.is_none());
    }
}
