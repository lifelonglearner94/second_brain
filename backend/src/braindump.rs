//! Braindump ingest + persistence (issue #5 / #42, ADR-0007).
//!
//! A braindump is an immutable thought-snapshot: verbatim (user-confirmed
//! text at submit), cleaned (LLM-produced rendering), and a timestamp. Edits
//! are error-correction only — they overwrite the verbatim in place and
//! re-clean; substantive thinking-evolution spawns a new braindump, never
//! edits the old one.
//!
//! This module owns the ingest pipeline — clean → persist → ontology →
//! extract → accrete (ADR-0007) — as [`ingest`] (submit) / [`ingest_edit`]
//! (error-correction). The HTTP handlers in [`crate::routes::braindump`] are
//! thin adapters (parse, validate non-empty, delegate, log); the pipeline is
//! unit-testable without an HTTP roundtrip. The accretion step delegates to
//! [`crate::graph::ingest_extraction`] (identity + provenance + type history +
//! embeddings, ADR-0001/0002/0003/0010).

use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::error::Result;
use crate::graph::{self, IngestOutcome};
use crate::graph_repo::{GraphRepo, SqliteGraphRepo};
use crate::llm::Llm;

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

/// Persist a new braindump with its verbatim and cleaned rendering (delegating
/// wrapper, issue #46). Returns the row as stored, with the surrogate id and
/// `created_at` filled in. Delegates to [`GraphRepo::insert_braindump`] on a
/// [`SqliteGraphRepo`].
pub async fn insert_braindump(
    db: &Db,
    user_id: &str,
    verbatim: &str,
    cleaned: &str,
) -> Result<Braindump> {
    SqliteGraphRepo::new(db.clone())
        .insert_braindump(user_id, verbatim, cleaned)
        .await
}

/// Look up a braindump by id. `None` if no row matches.
///
/// Delegates to [`GraphRepo::get_braindump`] (issue #48).
pub async fn get_braindump(db: &Db, user_id: &str, id: i64) -> Result<Option<Braindump>> {
    SqliteGraphRepo::new(db.clone())
        .get_braindump(user_id, id)
        .await
}

/// Overwrite the verbatim in place (error-correction, ADR-0007) and store the
/// re-cleaned rendering. The id and created_at are untouched — a braindump's
/// timestamp is its original submit instant, not its last edit. Returns the
/// updated row, or `None` if no braindump with `id` exists.
///
/// Delegates to [`GraphRepo::update_braindump`] (issue #48).
pub async fn overwrite_verbatim(
    db: &Db,
    user_id: &str,
    id: i64,
    verbatim: &str,
    cleaned: &str,
) -> Result<Option<Braindump>> {
    SqliteGraphRepo::new(db.clone())
        .update_braindump(user_id, id, verbatim.to_string(), cleaned.to_string())
        .await
}

/// The Braindump ingest pipeline for the submit path (ADR-0007): clean the
/// verbatim via the LLM seam, persist verbatim + cleaned + timestamp
/// immutably, then run extraction + atomic accretion (ontology → extract →
/// [`crate::graph::ingest_extraction`], ADR-0001/0002/0003/0010). Returns the
/// stored braindump and the accretion outcome so the caller can log it. This
/// is the spec — the sequence the `submit` HTTP handler delegates to, so the
/// pipeline is exercisable without an HTTP roundtrip. The edit path differs
/// only in its persist step; see [`ingest_edit`].
pub async fn ingest(
    db: &Db,
    user_id: &str,
    llm: &dyn Llm,
    verbatim: &str,
) -> Result<(Braindump, IngestOutcome)> {
    let cleaned = llm.clean(verbatim).await?;
    let braindump = insert_braindump(db, user_id, verbatim, &cleaned).await?;
    let outcome = accrete(db, user_id, llm, &braindump).await?;
    Ok((braindump, outcome))
}

/// The Braindump ingest pipeline for the edit path (ADR-0007 error-correction):
/// clean the corrected verbatim, overwrite it in place (id + created_at
/// untouched), then re-run extraction + accretion (the stale extraction is
/// retracted first inside [`crate::graph::ingest_extraction`]). Returns `None`
/// if no braindump with `id` exists — the caller (the `edit` HTTP handler)
/// maps that to `404`. Substantive thinking-evolution spawns a new braindump
/// via [`ingest`], never this.
pub async fn ingest_edit(
    db: &Db,
    user_id: &str,
    llm: &dyn Llm,
    id: i64,
    verbatim: &str,
) -> Result<Option<(Braindump, IngestOutcome)>> {
    let cleaned = llm.clean(verbatim).await?;
    let Some(braindump) = overwrite_verbatim(db, user_id, id, verbatim, &cleaned).await? else {
        return Ok(None);
    };
    let outcome = accrete(db, user_id, llm, &braindump).await?;
    Ok(Some((braindump, outcome)))
}

/// The shared post-persist core of the ingest pipeline (ADR-0007): load the
/// governed ontology, extract concepts + edges via the LLM seam, and run the
/// atomic accretion. This is the 90% that the `submit` and `edit` handlers
/// used to duplicate inline (issue #42); the only step that differs between
/// them is the persist call that produces the [`Braindump`] passed in here.
async fn accrete(
    db: &Db,
    user_id: &str,
    llm: &dyn Llm,
    braindump: &Braindump,
) -> Result<IngestOutcome> {
    let ontology = graph::ontology_slugs(db, user_id).await?;
    let extraction = llm.extract(&braindump.verbatim, &ontology).await?;
    graph::ingest_extraction(
        db,
        user_id,
        llm,
        braindump.id,
        &braindump.verbatim,
        extraction,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph;
    use crate::graph_repo::{GraphRepo, SqliteGraphRepo};
    use crate::llm::Llm;
    use std::sync::Arc;

    #[tokio::test]
    async fn insert_then_get_returns_stored_braindump() {
        let db = Db::open_in_memory().unwrap();
        let inserted =
            insert_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "hello world", "Hello, world.")
                .await
                .unwrap();
        let fetched = get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, inserted.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched, inserted);
        assert!(fetched.created_at > 0);
    }

    #[tokio::test]
    async fn get_missing_braindump_is_none() {
        let db = Db::open_in_memory().unwrap();
        let got = get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, 9999)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn overwrite_verbatim_replaces_in_place_keeping_id_and_timestamp() {
        let db = Db::open_in_memory().unwrap();
        let original = insert_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "hallo welt", "Hallo, Welt.")
            .await
            .unwrap();
        let updated = overwrite_verbatim(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            original.id,
            "hello world",
            "Hello, world.",
        )
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
        let refetched = get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, original.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refetched, updated);
    }

    #[tokio::test]
    async fn overwrite_verbatim_on_missing_id_is_none() {
        let db = Db::open_in_memory().unwrap();
        let got = overwrite_verbatim(&db, BOOTSTRAP_ADMIN_USER_ID, 9999, "x", "X")
            .await
            .unwrap();
        assert!(got.is_none());
    }

    // --- issue #42: braindump::ingest owns the full pipeline (no HTTP) ---

    /// In-memory Db with the vec0 embedding tables at the fake LLM's dim, so the
    /// accretion step can store/retrieve embeddings without an `AppState`.
    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(64).unwrap();
        db
    }

    /// An LLM scripted for the ingest pipeline: `clean` trims (the FakeLlm
    /// contract), `extract` returns a fixed concept+edge set so the accretion
    /// step has real work, and `embed_*` uses the deterministic token-bucket
    /// vector. Lets `ingest` run clean → persist → ontology → extract →
    /// accrete end-to-end with no network and no HTTP roundtrip.
    struct IngestLlm {
        result: ExtractionResult,
    }

    #[async_trait::async_trait]
    impl Llm for IngestLlm {
        async fn clean(&self, verbatim: &str) -> Result<String> {
            Ok(verbatim.trim().to_string())
        }
        async fn generate_pinned(&self, _: &str, user: &str) -> Result<String> {
            Ok(user.to_string())
        }
        async fn synthesize(&self, _: &str, _: &str) -> Result<String> {
            Ok("IngestLlm::synthesize (unused by ingest tests)".to_string())
        }
        async fn extract(&self, _: &str, _: &[String]) -> Result<ExtractionResult> {
            Ok(self.result.clone())
        }
        async fn embed_document(&self, text: &str) -> Result<Vec<f32>> {
            Ok(crate::embedding::deterministic_vector(text, 64))
        }
        async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
            Ok(crate::embedding::deterministic_vector(text, 64))
        }
        fn dim(&self) -> usize {
            64
        }
    }

    fn maria_endangers_q3() -> ExtractionResult {
        ExtractionResult {
            concepts: vec![
                ExtractedConcept {
                    label: "Maria".into(),
                },
                ExtractedConcept {
                    label: "Q3 launch".into(),
                },
            ],
            edges: vec![ExtractedEdge {
                from_label: "Maria".into(),
                type_slug: "endangers".into(),
                to_label: "Q3 launch".into(),
            }],
        }
    }

    #[tokio::test]
    async fn ingest_runs_clean_persist_ontology_extract_accrete_without_http() {
        let db = test_db();
        let llm = IngestLlm {
            result: maria_endangers_q3(),
        };
        let (braindump, outcome) = ingest(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            "  maria endangers the q3 launch  ",
        )
        .await
        .unwrap();

        // clean: cleaned is the trimmed verbatim (FakeLlm contract).
        assert_eq!(braindump.verbatim, "  maria endangers the q3 launch  ");
        assert_eq!(braindump.cleaned, "maria endangers the q3 launch");
        // persist: row fetchable by id with the cleaned rendering.
        assert!(braindump.id > 0);
        assert_eq!(
            get_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, braindump.id)
                .await
                .unwrap()
                .unwrap(),
            braindump
        );
        // ontology → extract → accrete: both concepts + the edge landed with
        // this braindump as their sole provenance, and the braindump embedding
        // was stored (accretion ran, not just the persist).
        assert_eq!(outcome.concepts_created, 2, "{outcome:?}");
        assert_eq!(outcome.edges_created, 1, "{outcome:?}");
        let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .unwrap();
        let q3 = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Q3 launch")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
                .await
                .unwrap(),
            vec![braindump.id]
        );
        assert_eq!(
            graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, q3)
                .await
                .unwrap(),
            vec![braindump.id]
        );
        let edge = graph::find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "endangers", q3)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            graph::edge_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap(),
            vec![braindump.id]
        );
        assert_eq!(
            graph::edge_type_history(&db, BOOTSTRAP_ADMIN_USER_ID, edge.id)
                .await
                .unwrap()
                .len(),
            1,
            "type history seeded at index 0 (ADR-0003)"
        );
        // The braindump-embedding check goes through the GraphRepo seam
        // (issue #44) rather than the direct DB closure call, proving the
        // adapter is reachable from a cross-module caller.
        let repo: Arc<dyn GraphRepo> = Arc::new(SqliteGraphRepo::new(db.clone()));
        assert!(
            repo.braindump_embedding_stored(BOOTSTRAP_ADMIN_USER_ID, braindump.id)
                .await
                .unwrap(),
            "braindump embedding stored (retrieval backfill)"
        );
    }

    #[tokio::test]
    async fn ingest_edit_overwrites_in_place_recleans_and_reaccretes_without_http() {
        let db = test_db();
        let llm = IngestLlm {
            result: maria_endangers_q3(),
        };
        let (first, first_outcome) = ingest(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            "maria endangers q3 launch",
        )
        .await
        .unwrap();
        assert_eq!(first_outcome.concepts_created, 2);

        // Edit: correct the verbatim. Same scripted extraction → Maria/Q3
        // accrete onto the existing nodes (no duplicates), id + timestamp
        // are stable, and the cleaned rendering is re-derived.
        let (edited, edited_outcome) = ingest_edit(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            first.id,
            "  maria endangers q3 launch again  ",
        )
        .await
        .unwrap()
        .expect("row exists");
        assert_eq!(edited.id, first.id, "id stable across edit (ADR-0007)");
        assert_eq!(
            edited.created_at, first.created_at,
            "timestamp stable across edit (ADR-0007)"
        );
        assert_eq!(edited.verbatim, "  maria endangers q3 launch again  ");
        assert_eq!(edited.cleaned, "maria endangers q3 launch again");
        // ADR-0007: the edit retracts the stale extraction first, then
        // re-accretes — so Maria/Q3 (whose only extractor was this braindump)
        // vanish and are re-created fresh, not accreted. The braindump id is
        // unchanged, so the re-created concepts carry provenance [first.id].
        assert_eq!(edited_outcome.concepts_created, 2, "{edited_outcome:?}");
        assert_eq!(edited_outcome.concepts_accreted, 0, "{edited_outcome:?}");
        assert_eq!(edited_outcome.edges_created, 1, "{edited_outcome:?}");
        let maria = graph::concept_id_for_label(&db, BOOTSTRAP_ADMIN_USER_ID, "Maria")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            graph::concept_provenance(&db, BOOTSTRAP_ADMIN_USER_ID, maria)
                .await
                .unwrap(),
            vec![first.id]
        );
    }

    #[tokio::test]
    async fn ingest_edit_on_missing_id_is_none() {
        let db = test_db();
        let llm = IngestLlm {
            result: ExtractionResult::default(),
        };
        assert!(
            ingest_edit(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, 9999, "x")
                .await
                .unwrap()
                .is_none(),
            "editing a missing braindump is None (caller maps to 404)"
        );
    }
}
