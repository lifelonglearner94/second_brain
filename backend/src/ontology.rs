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
//! pinned model snapshot via [`crate::llm::LlmClient::generate_pinned`].

use std::sync::Arc;

use rusqlite::{params, OptionalExtension};

use crate::db::{now_seconds, Db};
use crate::embedding::EmbeddingClient;
use crate::error::{Error, Result};
use crate::graph::{current_type_subquery, vec_to_blob};
use crate::llm::LlmClient;

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
pub async fn propose_type(
    db: &Db,
    embedding: &(dyn EmbeddingClient + Sync),
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
    let vec = embedding.embed_document(&text).await?;

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

    let created_at = now_seconds();
    let proposal = db
        .run(move |conn| {
            conn.execute(
                "INSERT INTO type_proposals
                    (slug, label, description, merge_of, status,
                     near_match_slug, near_match_similarity, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    slug,
                    label,
                    description,
                    merge_of,
                    status,
                    near_match_slug,
                    near_match_similarity,
                    created_at
                ],
            )?;
            Ok(TypeProposal {
                id: conn.last_insert_rowid(),
                slug,
                label,
                description,
                merge_of,
                status,
                near_match_slug,
                near_match_similarity,
                created_at,
                resolved_at: None,
            })
        })
        .await?;

    Ok(ProposeOutcome { proposal })
}

/// List all proposals, oldest first.
pub async fn list_proposals(db: &Db) -> Result<Vec<TypeProposal>> {
    db.run(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, slug, label, description, merge_of, status,
                    near_match_slug, near_match_similarity, created_at, resolved_at
             FROM type_proposals ORDER BY id",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TypeProposal {
                    id: r.get(0)?,
                    slug: r.get(1)?,
                    label: r.get(2)?,
                    description: r.get(3)?,
                    merge_of: r.get(4)?,
                    status: r.get(5)?,
                    near_match_slug: r.get(6)?,
                    near_match_similarity: r.get::<_, Option<f64>>(7)?.map(|f| f as f32),
                    created_at: r.get(8)?,
                    resolved_at: r.get(9)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    })
    .await
}

/// Look up a single proposal by id. `None` if no row matches.
pub async fn get_proposal(db: &Db, id: i64) -> Result<Option<TypeProposal>> {
    db.run(move |conn| {
        let row = conn
            .query_row(
                "SELECT id, slug, label, description, merge_of, status,
                        near_match_slug, near_match_similarity, created_at, resolved_at
                 FROM type_proposals WHERE id = ?1",
                params![id],
                |r| {
                    Ok(TypeProposal {
                        id: r.get(0)?,
                        slug: r.get(1)?,
                        label: r.get(2)?,
                        description: r.get(3)?,
                        merge_of: r.get(4)?,
                        status: r.get(5)?,
                        near_match_slug: r.get(6)?,
                        near_match_similarity: r.get::<_, Option<f64>>(7)?.map(|f| f as f32),
                        created_at: r.get(8)?,
                        resolved_at: r.get(9)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    })
    .await
}

/// Approve a pending proposal: add the type to the ontology, store its
/// type-embedding, mark the proposal `approved`, and (if `merge_of` is set)
/// enqueue an async refactor to retag existing edges of the merged type.
/// Returns the updated proposal. Errors with `Conflict` if the proposal is
/// not pending (already resolved or auto-merged).
pub async fn approve_proposal(
    db: &Db,
    embedding: &(dyn EmbeddingClient + Sync),
    id: i64,
) -> Result<TypeProposal> {
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
    let vec = embedding
        .embed_document(&type_text(
            &proposal.slug,
            &proposal.label,
            &proposal.description,
        ))
        .await?;
    let now = now_seconds();
    let slug = proposal.slug.clone();
    let label = proposal.label.clone();
    let description = proposal.description.clone();
    db.run(move |conn| {
        conn.execute_batch("BEGIN")?;
        match (|| -> Result<()> {
            conn.execute(
                "INSERT INTO ontology (slug, label, description, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![slug, label, description, now],
            )?;
            let ontology_id: i64 = conn.query_row(
                "SELECT id FROM ontology WHERE slug = ?1",
                params![slug],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO type_embeddings (ontology_id, embedding) VALUES (?1, ?2)",
                params![ontology_id, vec_to_blob(&vec)],
            )?;
            conn.execute(
                "UPDATE type_proposals SET status = 'approved', resolved_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
            Ok(())
        })() {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    })
    .await?;
    // Return the refreshed row.
    get_proposal(db, id)
        .await?
        .ok_or_else(|| Error::internal("proposal vanished after approve"))
}

/// Reject a pending proposal. Errors with `Conflict` if the proposal is not
/// pending. Idempotent over rejection: a second reject is a conflict.
pub async fn reject_proposal(db: &Db, id: i64) -> Result<TypeProposal> {
    let now = now_seconds();
    let updated = db
        .run(move |conn| {
            Ok(conn.execute(
                "UPDATE type_proposals SET status = 'rejected', resolved_at = ?1
                 WHERE id = ?2 AND status = 'pending'",
                params![now, id],
            )?)
        })
        .await?;
    if updated == 0 {
        // Either no such proposal, or it exists but isn't pending. Distinguish.
        match get_proposal(db, id).await? {
            None => Err(Error::NotFound(format!("type proposal {id} not found"))),
            Some(p) => Err(Error::Conflict(format!(
                "proposal {id} is `{}`, not `pending` — cannot reject",
                p.status
            ))),
        }
    } else {
        get_proposal(db, id)
            .await?
            .ok_or_else(|| Error::internal("proposal vanished after reject"))
    }
}

// --- read helpers (current type projection, edge selection for refactor) ---

/// The projected current type of an edge: the last entry of its append-only
/// type history (ADR-0003). `None` if the edge has no type history (should not
/// happen for a real edge — index 0 is initialised at creation).
pub async fn current_edge_type(db: &Db, edge_id: i64) -> Result<Option<String>> {
    db.run(move |conn| {
        let slug = conn
            .query_row(
                "SELECT type_slug FROM edge_type_history
                 WHERE edge_id = ?1 ORDER BY seq_index DESC LIMIT 1",
                params![edge_id],
                |r| r.get::<_, String>(0),
            )
            .optional()?;
        Ok(slug)
    })
    .await
}

/// Every edge id whose projected current type is `slug`. The refactor targets
/// these edges when `slug` is the `merge_of` of an approved proposal.
pub async fn edges_with_current_type(db: &Db, slug: &str) -> Result<Vec<i64>> {
    let slug = slug.to_string();
    db.run(move |conn| {
        let mut stmt = conn.prepare(&format!(
            "SELECT e.id FROM edges e WHERE ({}) = ?1 ORDER BY e.id",
            current_type_subquery()
        ))?;
        let ids = stmt
            .query_map(params![slug], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(ids)
    })
    .await
}

// --- type-embedding KNN ---

/// sqlite-vec KNN: nearest type by cosine. Returns `(slug, similarity)` where
/// similarity = 1 − distance. `None` if the type-embedding collection is empty.
///
/// The KNN runs against the vec0 table directly (sqlite-vec's MATCH planner
/// requires the vec0 table be the query's FROM source, not hidden behind a
/// JOIN); the slug is looked up by the returned `ontology_id` afterwards.
pub async fn knn_type(db: &Db, query_vec: &[f32]) -> Result<Option<(String, f32)>> {
    let blob = vec_to_blob(query_vec);
    db.run(move |conn| {
        let row = conn
            .prepare(
                "SELECT ontology_id, distance FROM type_embeddings
                 WHERE embedding MATCH ?1 ORDER BY distance LIMIT 1",
            )?
            .query_row(params![blob], |r| {
                Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
            })
            .optional()?;
        match row {
            Some((ontology_id, sim)) => {
                let slug: String = conn.query_row(
                    "SELECT slug FROM ontology WHERE id = ?1",
                    params![ontology_id],
                    |r| r.get(0),
                )?;
                Ok(Some((slug, sim)))
            }
            None => Ok(None),
        }
    })
    .await
}

/// Whether a slug already exists in the ontology (connection-scoped helper
/// for use inside `db.run` closures).
pub(crate) fn ontology_slug_exists_conn(
    conn: &rusqlite::Connection,
    slug: &str,
) -> Result<bool> {
    let n: i64 = conn.query_row(
        "SELECT COUNT(*) FROM ontology WHERE slug = ?1",
        params![slug],
        |r| r.get(0),
    )?;
    Ok(n > 0)
}

/// Whether a slug already exists in the ontology.
pub async fn ontology_slug_exists(db: &Db, slug: &str) -> Result<bool> {
    let slug = slug.to_string();
    db.run(move |conn| ontology_slug_exists_conn(conn, &slug)).await
}

/// All ontology types as `(slug, label, description)`, ordered by `id`. Used
/// to seed type-embeddings and by tests to construct `type_text` for dedup.
pub async fn ontology_types(db: &Db) -> Result<Vec<(String, String, String)>> {
    db.run(|conn| {
        let mut stmt = conn.prepare("SELECT slug, label, description FROM ontology ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    })
    .await
}

/// Embed every ontology type not yet in the `type_embeddings` collection and
/// store the result. Idempotent: types already embedded are skipped. Called at
/// startup so the seeded day-zero vocabulary has embeddings for dedup before
/// the first proposal arrives.
pub async fn seed_type_embeddings(
    db: &Db,
    embedding: &(dyn EmbeddingClient + Sync),
) -> Result<usize> {
    // Load all (id, slug, label, description) for types missing an embedding.
    let missing: Vec<(i64, String, String, String)> = db
        .run(|conn| {
            let mut stmt = conn.prepare(
                "SELECT o.id, o.slug, o.label, o.description FROM ontology o
                 WHERE NOT EXISTS
                     (SELECT 1 FROM type_embeddings t WHERE t.ontology_id = o.id)
                 ORDER BY o.id",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
        .await?;

    let mut count = 0;
    for (ontology_id, slug, label, description) in missing {
        let vec = embedding
            .embed_document(&type_text(&slug, &label, &description))
            .await?;
        let ontology_id_capture = ontology_id;
        db.run(move |conn| {
            conn.execute(
                "INSERT OR IGNORE INTO type_embeddings (ontology_id, embedding)
                 VALUES (?1, ?2)",
                params![ontology_id_capture, vec_to_blob(&vec)],
            )?;
            Ok(())
        })
        .await?;
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
/// [`LlmClient::generate_pinned`] so retagging is deterministic and stable
/// across API model bumps. No-op when the proposal has no `merge_of` (pure new
/// type) or when no edges currently wear the merged type.
///
/// Public so tests can drive it synchronously; the route spawns it via
/// `tokio::spawn` so ingest is not blocked while it runs.
pub async fn run_refactor(
    db: &Db,
    llm: &(dyn LlmClient + Sync),
    proposal: &TypeProposal,
) -> Result<RefactorOutcome> {
    let Some(merge_of) = proposal.merge_of.as_ref() else {
        return Ok(RefactorOutcome::default());
    };
    let edge_ids = edges_with_current_type(db, merge_of).await?;
    if edge_ids.is_empty() {
        return Ok(RefactorOutcome::default());
    }

    let ontology = crate::graph::ontology_slugs(db).await?;
    let new_slug = proposal.slug.clone();
    let system = "You re-classify edges when the ontology evolves. \
                  Given an edge and the new vocabulary, respond with the single slug \
                  that best fits the edge now. Respond with only the slug, nothing else.";
    let merge_of_for_prompt = merge_of.clone();
    let label_for_prompt = proposal.label.clone();
    let description_for_prompt = proposal.description.clone();

    // Re-classify each affected edge. The LLM call is a network round-trip, so
    // it runs outside the SQLite transaction; the resulting type-history
    // appends commit atomically together (ADR-0003: a partial refactor would
    // leave the graph mid-migration).
    let mut retagged: Vec<(i64, String)> = Vec::with_capacity(edge_ids.len());
    for edge_id in edge_ids {
        let (source_label, target_label, current_type) =
            edge_endpoints_and_type(db, edge_id).await?;
        let user = format!(
            "Edge: {source_label} —[{current_type}]→ {target_label}\n\
             The type `{merge_of_for_prompt}` has been merged into `{new_slug}` \
             (label: {label_for_prompt}; description: {description_for_prompt}).\n\
             Re-classify this edge. Respond with exactly one slug from: [{}].",
            ontology.join(", ")
        );
        let response = llm.generate_pinned(system, &user).await?;
        let slug = response.trim().to_string();
        // Default to the new slug if the LLM hallucinated; only sanctioned slugs
        // may be written to the type history (ADR-0002: the LLM never invents a
        // type).
        let chosen = if ontology.iter().any(|s| s == &slug) {
            slug
        } else {
            tracing::warn!(
                edge_id,
                raw = %slug,
                "refactor LLM returned an unsanctioned slug; defaulting to the new type"
            );
            new_slug.clone()
        };
        retagged.push((edge_id, chosen));
    }

    let edges_retagged = retagged.len();
    db.run(move |conn| {
        conn.execute_batch("BEGIN")?;
        match (|| -> Result<()> {
            for (edge_id, slug) in &retagged {
                append_type_history_conn(conn, *edge_id, slug)?;
            }
            Ok(())
        })() {
            Ok(()) => {
                conn.execute_batch("COMMIT")?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    })
    .await?;

    Ok(RefactorOutcome { edges_retagged })
}

/// Append a new type-history entry to an edge (ADR-0003). `seq_index` is
/// `max(existing) + 1`, so refactors stack without overwriting.
fn append_type_history_conn(
    conn: &rusqlite::Connection,
    edge_id: i64,
    type_slug: &str,
) -> Result<()> {
    let created_at = now_seconds();
    let next_seq: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(seq_index), -1) + 1 FROM edge_type_history WHERE edge_id = ?1",
            params![edge_id],
            |r| r.get(0),
        )
        .unwrap_or(0);
    conn.execute(
        "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![edge_id, next_seq, type_slug, created_at],
    )?;
    Ok(())
}

/// The (source label, target label, current type) for an edge — the prompt
/// payload for the refactor LLM.
async fn edge_endpoints_and_type(db: &Db, edge_id: i64) -> Result<(String, String, String)> {
    db.run(move |conn| {
        let row = conn.query_row(
            &format!(
                "SELECT sc.label, tc.label, ({})
                 FROM edges e
                 JOIN concepts sc ON sc.id = e.source_concept_id
                 JOIN concepts tc ON tc.id = e.target_concept_id
                 WHERE e.id = ?1",
                current_type_subquery()
            ),
            params![edge_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )?;
        Ok(row)
    })
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
    pub fn spawn(&self, db: Db, llm: Arc<dyn LlmClient>, proposal: TypeProposal) {
        let handle = tokio::spawn(async move {
            let outcome = run_refactor(&db, llm.as_ref(), &proposal).await;
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
    use crate::embedding::FakeEmbedding;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeEmbedding::default().dim())
            .unwrap();
        db
    }

    fn fake_embedding() -> FakeEmbedding {
        FakeEmbedding::default()
    }

    #[tokio::test]
    async fn propose_with_no_existing_type_embeddings_is_pending() {
        let db = test_db();
        let emb = fake_embedding();
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        assert_eq!(out.proposal.status, "pending");
        assert_eq!(out.proposal.slug, "nurtures");
        assert!(out.proposal.near_match_slug.is_none());
    }

    #[tokio::test]
    async fn propose_with_empty_slug_is_bad_request() {
        let db = test_db();
        let emb = fake_embedding();
        let err = propose_type(&db, &emb, "  ", "X", "desc", None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_with_existing_slug_is_bad_request() {
        let db = test_db();
        let emb = fake_embedding();
        let err = propose_type(&db, &emb, "causes", "Causes", "dup", None)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::BadRequest(_)), "{err:?}");
    }

    #[tokio::test]
    async fn propose_with_unknown_merge_of_is_bad_request() {
        let db = test_db();
        let emb = fake_embedding();
        let err = propose_type(
            &db,
            &emb,
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
        let emb = fake_embedding();
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = approve_proposal(&db, &emb, out.proposal.id).await.unwrap();
        assert_eq!(proposal.status, "approved");
        assert!(ontology_slug_exists(&db, "nurtures").await.unwrap());
        let near = knn_type(
            &db,
            &emb.embed_document("nurtures Nurtures A nurtures B.")
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
        let emb = fake_embedding();
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        approve_proposal(&db, &emb, out.proposal.id).await.unwrap();
        let err = approve_proposal(&db, &emb, out.proposal.id)
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err:?}");
    }

    #[tokio::test]
    async fn reject_marks_pending_proposal_rejected() {
        let db = test_db();
        let emb = fake_embedding();
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = reject_proposal(&db, out.proposal.id).await.unwrap();
        assert_eq!(proposal.status, "rejected");
        assert!(!ontology_slug_exists(&db, "nurtures").await.unwrap());
    }

    #[tokio::test]
    async fn reject_already_resolved_is_conflict() {
        let db = test_db();
        let emb = fake_embedding();
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
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
        db.run(|conn| {
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
        db.run(|conn| {
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
        db.run(|conn| {
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
        let emb = fake_embedding();
        let llm = crate::llm::FakeLlm;
        let out = propose_type(&db, &emb, "nurtures", "Nurtures", "A nurtures B.", None)
            .await
            .unwrap();
        let proposal = approve_proposal(&db, &emb, out.proposal.id).await.unwrap();
        let outcome = run_refactor(&db, &llm, &proposal).await.unwrap();
        assert_eq!(outcome.edges_retagged, 0);
    }

    #[tokio::test]
    async fn run_refactor_with_merge_of_and_no_edges_is_noop() {
        let db = test_db();
        let emb = fake_embedding();
        let llm = crate::llm::FakeLlm;
        let out = propose_type(
            &db,
            &emb,
            "nurtures",
            "Nurtures",
            "A nurtures B.",
            Some("helps"),
        )
        .await
        .unwrap();
        let proposal = approve_proposal(&db, &emb, out.proposal.id).await.unwrap();
        let outcome = run_refactor(&db, &llm, &proposal).await.unwrap();
        assert_eq!(outcome.edges_retagged, 0);
    }
}
