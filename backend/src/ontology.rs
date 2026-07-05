//! Ontology governance (issue #9, ADR-0003): propose/approve new edge types,
//! type-embedding dedup (above 99.5% auto-merge else queued), and the async
//! refactor job that retags existing edges via the append-only type history.
//!
//! The fractal-tolerance threshold (above 99.5%) is stricter than concept
//! identity's above-95% (ADR-0001): a wrong type merge is a schema error that
//! corrupts every edge using that type, so obvious 1:1 duplicates are
//! auto-merged and everything riskier goes to a human curation queue.
//!
//! On approval of a `merge_of` proposal, an async refactor retags each affected
//! edge by *appending* to its `edge_type_history` (never overwriting — ADR-0003:
//! index 0 is the immutable original assertion, the current type is the
//! projection of the last entry). The refactor runs at Temperature=0 against a
//! pinned model snapshot via [`crate::llm::Llm::generate_pinned`].
//!
//! After #47 the governance SQL (propose/approve/reject, current-type
//! projection, edge selection, refactor retag) lives behind the [`GraphRepo`]
//! trait — in [`SqliteGraphRepo`]'s impl — so the full flow is testable via
//! [`InMemoryGraphRepo`] without SQLite. The free functions here are thin
//! delegating wrappers: they own the LLM embedding computation and validation,
//! then delegate the pure-DB storage work to the trait. `seed_type_embeddings`
//! stays as direct SQL — it is a startup seed, not a governance operation.

use std::sync::Arc;

use crate::db::Db;
use crate::error::{Error, Result};
use crate::graph_repo::{GraphRepo, SqliteGraphRepo};
use crate::llm::Llm;

/// Cosine similarity at or above which a proposed type is deemed a 1:1
/// duplicate of an existing type and auto-merged (ADR-0003: >99.5%). Stricter
/// than concept identity's 95% (ADR-0001) because a wrong type merge corrupts
/// every edge using that type.
pub const TYPE_MERGE_THRESHOLD: f32 = 0.995;

/// The text canonicalised for type-embedding — slug + label + description — so
/// dedup compares the full type, not just the slug.
pub fn type_text(slug: &str, label: &str, description: &str) -> String {
    format!("{slug} {label} {description}")
}

/// One row in the `type_proposals` queue. `status` is `pending`,
/// `auto_merged`, `approved`, or `rejected`. `merge_of` is the ontology slug
/// the proposed type replaces (set on approve → triggers the refactor); `None`
/// for a pure new type.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TypeProposal {
    pub id: i64,
    pub slug: String,
    pub label: String,
    pub description: String,
    pub merge_of: Option<String>,
    pub status: String,
    pub near_match_slug: Option<String>,
    pub near_match_similarity: Option<f32>,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

use serde::Serialize;

/// Outcome of a propose call: either the proposal was queued for human review
/// (`pending`) or auto-merged into an existing type (`auto_merged`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProposeOutcome {
    pub proposal: TypeProposal,
}

/// What the refactor job did: how many edges it retagged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RefactorOutcome {
    pub edges_retagged: usize,
}

/// Propose a new edge type. Embeds the type (slug + label + description) and
/// KNN-searches the type-embedding collection: above [`TYPE_MERGE_THRESHOLD`]
/// the proposal is `auto_merged` (the duplicate is not added to the ontology);
/// otherwise it is `pending` and queued for human review. If no type-embeddings
/// exist yet (empty ontology at first run), the proposal is `pending`.
///
/// Wrapper: the LLM embedding and KNN run here; the INSERT is delegated to
/// [`GraphRepo::insert_type_proposal`] (issue #47).
pub async fn propose_type(
    db: &Db,
    llm: &dyn Llm,
    slug: &str,
    label: &str,
    description: &str,
    merge_of: Option<&str>,
) -> Result<ProposeOutcome> {
    let slug = slug.trim().to_string();
    let label = label.trim().to_string();
    let description = description.trim().to_string();
    if slug.is_empty() || label.is_empty() || description.is_empty() {
        return Err(Error::BadRequest(
            "slug, label, and description must be non-empty".into(),
        ));
    }
    // Reject a proposal whose slug already exists in the ontology — the user
    // should edit the existing type instead.
    if ontology_slug_exists(db, &slug).await? {
        return Err(Error::BadRequest(format!(
            "type slug `{slug}` already exists in the ontology"
        )));
    }
    let merge_of = merge_of.map(|s| s.trim().to_string());
    if let Some(ref m) = merge_of {
        if !ontology_slug_exists(db, m).await? {
            return Err(Error::BadRequest(format!(
                "merge_of slug `{m}` does not exist in the ontology"
            )));
        }
    }

    let text = type_text(&slug, &label, &description);
    let vec = llm.embed_document(&text).await?;

    let near = knn_type(db, &vec).await?;
    let (status, near_match_slug, near_match_similarity) = match near {
        Some((near_slug, sim)) if sim >= TYPE_MERGE_THRESHOLD => {
            ("auto_merged".to_string(), Some(near_slug), Some(sim))
        }
        _ => {
            let (slug, sim) = near.unzip();
            ("pending".to_string(), slug, sim)
        }
    };

    let proposal = SqliteGraphRepo::new(db.clone())
        .insert_type_proposal(
            slug,
            label,
            description,
            merge_of,
            status,
            near_match_slug,
            near_match_similarity,
        )
        .await?;

    Ok(ProposeOutcome { proposal })
}

/// List all proposals, oldest first.
///
/// Wrapper: delegates to [`GraphRepo::list_type_proposals`] (issue #47).
pub async fn list_proposals(db: &Db) -> Result<Vec<TypeProposal>> {
    SqliteGraphRepo::new(db.clone()).list_type_proposals().await
}

/// Look up a single proposal by id. `None` if no row matches.
///
/// Wrapper: delegates to [`GraphRepo::get_type_proposal`] (issue #47).
pub async fn get_proposal(db: &Db, id: i64) -> Result<Option<TypeProposal>> {
    SqliteGraphRepo::new(db.clone()).get_type_proposal(id).await
}

/// Approve a pending proposal: add the type to the ontology, store its
/// type-embedding, mark the proposal `approved`, and (if `merge_of` is set)
/// enqueue an async refactor to retag existing edges of the merged type.
/// Returns the updated proposal. Errors with `Conflict` if the proposal is
/// not pending (already resolved or auto-merged).
///
/// Wrapper: the LLM embedding runs here; the atomic INSERT+UPDATE is delegated
/// to [`GraphRepo::approve_type_proposal`] (issue #47).
pub async fn approve_proposal(db: &Db, llm: &dyn Llm, id: i64) -> Result<TypeProposal> {
    // Load first so we can validate status and compute the embedding text
    // outside the transaction (the network call cannot live in a sync SQLite
    // transaction — same shape as `ingest_extraction`).
    let proposal = get_proposal(db, id)
        .await?
        .ok_or_else(|| Error::NotFound(format!("type proposal {id} not found")))?;
    if proposal.status != "pending" {
        return Err(Error::Conflict(format!(
            "proposal {id} is `{}`, not `pending` — cannot approve",
            proposal.status
        )));
    }
    let vec = llm
        .embed_document(&type_text(
            &proposal.slug,
            &proposal.label,
            &proposal.description,
        ))
        .await?;
    SqliteGraphRepo::new(db.clone())
        .approve_type_proposal(
            id,
            proposal.slug.clone(),
            proposal.label.clone(),
            proposal.description.clone(),
            vec,
        )
        .await?;
    // Return the refreshed row.
    get_proposal(db, id)
        .await?
        .ok_or_else(|| Error::internal("proposal vanished after approve"))
}

/// Reject a pending proposal. Errors with `Conflict` if the proposal is not
/// pending. Idempotent over rejection: a second reject is a conflict.
///
/// Wrapper: delegates to [`GraphRepo::reject_type_proposal`] (issue #47) which
/// owns the pending-guard and the NotFound/Conflict distinction.
pub async fn reject_proposal(db: &Db, id: i64) -> Result<TypeProposal> {
    SqliteGraphRepo::new(db.clone())
        .reject_type_proposal(id)
        .await?;
    get_proposal(db, id)
        .await?
        .ok_or_else(|| Error::internal("proposal vanished after reject"))
}

// --- read helpers (current type projection, edge selection for refactor) ---

/// The projected current type of an edge: the last entry of its append-only
/// type history (ADR-0003). `None` if the edge has no type history (should not
/// happen for a real edge — index 0 is initialised at creation).
///
/// Wrapper: delegates to [`GraphRepo::current_edge_type`] (issue #47).
pub async fn current_edge_type(db: &Db, edge_id: i64) -> Result<Option<String>> {
    SqliteGraphRepo::new(db.clone())
        .current_edge_type(edge_id)
        .await
}

/// Every edge id whose projected current type is `slug`. The refactor targets
/// these edges when `slug` is the `merge_of` of an approved proposal.
///
/// Wrapper: delegates to [`GraphRepo::edges_with_current_type`] (issue #47).
pub async fn edges_with_current_type(db: &Db, slug: &str) -> Result<Vec<i64>> {
    SqliteGraphRepo::new(db.clone())
        .edges_with_current_type(slug)
        .await
}

// --- type-embedding KNN ---

/// sqlite-vec KNN: nearest type by cosine. Returns `(slug, similarity)` where
/// similarity = 1 − distance. `None` if the type-embedding collection is empty.
///
/// The KNN runs against the vec0 table directly (sqlite-vec's MATCH planner
/// requires the vec0 table be the query's FROM source, not hidden behind a
/// JOIN); the slug is looked up by the returned `ontology_id` afterwards.
///
/// Delegates to the [`GraphRepo`] trait (issue #45); the SQL lives in
/// [`SqliteGraphRepo`]'s trait impl so the read model stays hermetic. Stays
/// as a free function so `propose_type` (which takes `&Db`, not a repo) and
/// the integration tests under `backend/tests/` keep compiling; #48 removes
/// it once every caller is migrated.
pub async fn knn_type(db: &Db, query_vec: &[f32]) -> Result<Option<(String, f32)>> {
    SqliteGraphRepo::new(db.clone()).knn_type(query_vec).await
}

/// Whether a slug already exists in the ontology.
///
/// Wrapper: delegates to [`GraphRepo::ontology_slugs`] (issue #45) and checks
/// membership — a governance read, not a hot path.
pub async fn ontology_slug_exists(db: &Db, slug: &str) -> Result<bool> {
    let slug = slug.to_string();
    let slugs = SqliteGraphRepo::new(db.clone()).ontology_slugs().await?;
    Ok(slugs.contains(&slug))
}

/// All ontology types as `(slug, label, description)`, ordered by `id`. Used
/// to seed type-embeddings and by tests to construct `type_text` for dedup.
///
/// Delegates to the [`GraphRepo`] trait (issue #45); the SQL lives in
/// [`SqliteGraphRepo`]'s trait impl — the duplicated `SELECT slug, label,
/// description FROM ontology ORDER BY id` query exists in exactly one place
/// now (the Sqlite adapter). Stays as a free function so the integration
/// tests under `backend/tests/` keep compiling; #48 removes it.
pub async fn ontology_types(db: &Db) -> Result<Vec<(String, String, String)>> {
    SqliteGraphRepo::new(db.clone()).ontology_types().await
}

/// Embed every ontology type not yet in the `type_embeddings` collection and
/// store the result. Idempotent: types already embedded are skipped. Called at
/// startup so the seeded day-zero vocabulary has embeddings for dedup before
/// the first proposal arrives.
///
/// Delegates the DB reads/writes to [`GraphRepo::missing_type_rows`] +
/// [`GraphRepo::store_type_embedding`] (issue #48); the LLM embedding
/// computation runs here (same pattern as `ingest_extraction`: LLM in the
/// wrapper, trait is pure-DB).
pub async fn seed_type_embeddings(db: &Db, llm: &dyn Llm) -> Result<usize> {
    let repo = SqliteGraphRepo::new(db.clone());
    let missing = repo.missing_type_rows().await?;

    let mut count = 0;
    for (ontology_id, slug, label, description) in missing {
        let vec = llm
            .embed_document(&type_text(&slug, &label, &description))
            .await?;
        repo.store_type_embedding(ontology_id, vec).await?;
        count += 1;
    }
    Ok(count)
}

// --- the async refactor ---

/// Run the ontology refactor for an approved proposal: re-classify every edge
/// whose current type is the proposal's `merge_of` against the new vocabulary,
/// appending the LLM's chosen slug to each edge's type history (ADR-0003).
///
/// Runs at Temperature=0 against a pinned model snapshot via
/// [`Llm::generate_pinned`] so retagging is deterministic and stable
/// across API model bumps. No-op when the proposal has no `merge_of` (pure new
/// type) or when no edges currently wear the merged type.
///
/// Public so tests can drive it synchronously; the route spawns it via
/// `tokio::spawn` so ingest is not blocked while it runs.
///
/// Wrapper: delegates to [`GraphRepo::run_refactor`] (issue #47). The trait
/// method takes `&dyn Llm` (allowed deviation: the LLM-per-edge interleaving
/// can't be split without duplicating logic); the InMemoryGraphRepo impl
/// works with FakeLlm so tests are hermetic.
pub async fn run_refactor(
    db: &Db,
    llm: &dyn Llm,
    proposal: &TypeProposal,
) -> Result<RefactorOutcome> {
    SqliteGraphRepo::new(db.clone())
        .run_refactor(llm, proposal)
        .await
}

// --- the background refactor runner ---

/// A handle to the in-flight refactor spawned by an approve route, so tests
/// can await it deterministically without sleeping. Production code fires and
/// forgets — the spawned task runs out-of-band and ingest is not blocked.
///
/// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because the lock is only
/// held briefly to push a `JoinHandle` and never across an `.await` — so a
/// sync lock is correct and never blocks the async runtime meaningfully.
///
/// After #47, `spawn` takes `Arc<dyn GraphRepo>` (not `Db`) so the refactor
/// runs against any adapter — production wires `SqliteGraphRepo`, tests wire
/// `InMemoryGraphRepo`.
#[derive(Clone, Default)]
pub struct RefactorRunner {
    inner: Arc<std::sync::Mutex<Vec<tokio::task::JoinHandle<Result<RefactorOutcome>>>>>,
}

impl RefactorRunner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawn a refactor in the background. The route returns immediately; the
    /// job commits its type-history appends when it completes.
    pub fn spawn(&self, repo: Arc<dyn GraphRepo>, llm: Arc<dyn Llm>, proposal: TypeProposal) {
        let handle = tokio::spawn(async move {
            let outcome = repo.run_refactor(llm.as_ref(), &proposal).await;
            if let Err(e) = &outcome {
                tracing::error!(error = %e, "ontology refactor failed");
            }
            outcome
        });
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(handle);
        }
    }

    /// Await every in-flight refactor (test-only seam — production never waits
    /// on the background job).
    pub async fn await_all(&self) {
        let handles = {
            let mut guard = self.inner.lock().expect("refactor runner mutex poisoned");
            std::mem::take(&mut *guard)
        };
        for h in handles {
            let _ = h.await;
        }
    }
}

/// Test seam: await every in-flight refactor spawned by the approve route on
/// this state. Production code never calls this — the refactor runs
/// out-of-band.
pub async fn await_pending_refactors(state: &crate::state::AppState) {
    state.refactor_runner.await_all().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{FakeLlm, Llm};
    use rusqlite::params;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeLlm::default().dim()).unwrap();
        db
    }

    fn fake_llm() -> FakeLlm {
        FakeLlm::default()
    }

    #[tokio::test]
    async fn propose_with_no_existing_type_embeddings_is_pending() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        assert_eq!(out.proposal.status, "pending");
        assert_eq!(out.proposal.slug, "nurtures");
        assert!(out.proposal.near_match_slug.is_none());
    }

    #[tokio::test]
    async fn propose_with_empty_slug_is_bad_request() {
        let db = test_db();
        let llm = fake_llm();
        let err = propose_type(&db, &llm, "  ", "X", "desc", None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_with_existing_slug_is_bad_request() {
        let db = test_db();
        let llm = fake_llm();
        let err = propose_type(&db, &llm, "causes", "Causes", "dup", None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_with_unknown_merge_of_is_bad_request() {
        let db = test_db();
        let llm = fake_llm();
        let err = propose_type(
            &db,
            &llm,
            "nurtures",
            "Nurtures",
            "desc",
            Some("nonexistent_slug"),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn approve_adds_type_to_ontology_and_stores_embedding() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = approve_proposal(&db, &llm, out.proposal.id).await.unwrap();
        assert_eq!(proposal.status, "approved");
        assert!(ontology_slug_exists(&db, "nurtures").await.unwrap());
        let near = knn_type(
            &db,
            &llm.embed_document("nurtures Nurtures A nurtures B.")
                .await
                .unwrap(),
        )
        .await
        .unwrap();
        assert!(near.is_some());
        assert_eq!(near.unwrap().0, "nurtures");
    }

    #[tokio::test]
    async fn approve_already_resolved_is_conflict() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        approve_proposal(&db, &llm, out.proposal.id).await.unwrap();
        let err = approve_proposal(&db, &llm, out.proposal.id)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_marks_pending_proposal_rejected() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = reject_proposal(&db, out.proposal.id).await.unwrap();
        assert_eq!(proposal.status, "rejected");
        assert!(!ontology_slug_exists(&db, "nurtures").await.unwrap());
    }

    #[tokio::test]
    async fn reject_already_resolved_is_conflict() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        reject_proposal(&db, out.proposal.id).await.unwrap();
        let err = reject_proposal(&db, out.proposal.id).await.unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_missing_proposal_is_not_found() {
        let db = test_db();
        let err = reject_proposal(&db, 9999).await.unwrap_err();
        assert!(matches!(err, Error::NotFound(_)), "{err:?}");
    }

    #[tokio::test]
    async fn current_edge_type_projects_last_history_entry() {
        // Directly verify the projection: an edge with two type-history entries
        // projects the second one as its current type.
        let db = test_db();
        // Build a minimal edge + history by hand.
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO braindumps (verbatim, cleaned, created_at) VALUES ('v', 'c', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO concepts (label, created_at) VALUES ('A', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO concepts (label, created_at) VALUES ('B', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO edges (source_concept_id, target_concept_id, original_type, created_at)
                 VALUES (1, 2, 'helps', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (1, 0, 'helps', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            current_edge_type(&db, 1).await.unwrap().as_deref(),
            Some("helps")
        );
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (1, 1, 'nurtures', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            current_edge_type(&db, 1).await.unwrap().as_deref(),
            Some("nurtures"),
            "projection reads the last entry"
        );
    }

    #[tokio::test]
    async fn edges_with_current_type_finds_only_edges_wearing_that_type() {
        let db = test_db();
        db.with_conn(|conn| {
            conn.execute(
                "INSERT INTO braindumps (verbatim, cleaned, created_at) VALUES ('v', 'c', 0)",
                [],
            )?;
            for label in ["A", "B", "C"] {
                conn.execute(
                    "INSERT INTO concepts (label, created_at) VALUES (?1, 0)",
                    params![label],
                )?;
            }
            // Edge 1: helps (not retagged).
            conn.execute(
                "INSERT INTO edges (source_concept_id, target_concept_id, original_type, created_at)
                 VALUES (1, 2, 'helps', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (1, 0, 'helps', 0)",
                [],
            )?;
            // Edge 2: helps → nurtures (refactored; current type is nurtures).
            conn.execute(
                "INSERT INTO edges (source_concept_id, target_concept_id, original_type, created_at)
                 VALUES (1, 3, 'helps', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (2, 0, 'helps', 0)",
                [],
            )?;
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (2, 1, 'nurtures', 0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let helps = edges_with_current_type(&db, "helps").await.unwrap();
        let nurtures = edges_with_current_type(&db, "nurtures").await.unwrap();
        assert_eq!(helps, vec![1], "only the un-refactored edge: {helps:?}");
        assert_eq!(
            nurtures,
            vec![2],
            "the refactored edge projects to nurtures: {nurtures:?}"
        );
    }

    #[tokio::test]
    async fn run_refactor_with_no_merge_of_is_noop() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(&db, &llm, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = approve_proposal(&db, &llm, out.proposal.id).await.unwrap();
        let outcome = run_refactor(&db, &llm, &proposal).await.unwrap();
        assert_eq!(outcome.edges_retagged, 0);
    }

    #[tokio::test]
    async fn run_refactor_with_merge_of_and_no_edges_is_noop() {
        let db = test_db();
        let llm = fake_llm();
        let out = propose_type(
            &db,
            &llm,
            "nurtures",
            "Nurtures",
            "A nurtures B.",
            Some("helps"),
        )
        .await
        .unwrap();
        let proposal = approve_proposal(&db, &llm, out.proposal.id).await.unwrap();
        let outcome = run_refactor(&db, &llm, &proposal).await.unwrap();
        assert_eq!(outcome.edges_retagged, 0);
    }
}
