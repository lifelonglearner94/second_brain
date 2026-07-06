//! The retrieval read path (issue #8, ADR-0004).
//!
//! Seed-then-expand: the graph is load-bearing, vectors are seed and backfill.
//! A query is Gemini-embedded (query task type), sqlite-vec KNN over
//! concept-embeddings seeds the entry concept(s), petgraph traversal along typed
//! edges expands the neighbourhood, braindumps collected from the subgraph — plus
//! braindump-embedding backfill for strays the graph missed — form the context.
//!
//! Unanchored queries with no concept seed fall back to braindump-vector-direct
//! — the one place vectors become primary rather than seed/backfill.
//!
//! After #47 the full pipeline (KNN seed + BFS expand + collect + backfill) is
//! behind the [`GraphRepo::retrieve`] trait method. The petgraph BFS is private
//! to the retrieval implementation in `graph_repo.rs` — no other module imports
//! petgraph. The LLM query-embedding runs in this wrapper; the trait method
//! takes the precomputed `query_vec` (following #46's pattern: LLM in the
//! wrapper, trait is pure-DB).

use serde::Serialize;

use crate::db::Db;
use crate::error::Result;
use crate::graph_repo::{GraphRepo, SqliteGraphRepo};
use crate::llm::Llm;

/// Cosine similarity at or above which a concept-embedding KNN hit counts as a
/// retrieval seed. Below this the query is treated as unanchored and retrieval
/// falls back to braindump-vector-direct (ADR-0004 no-seed fallback).
pub const SEED_SIMILARITY_FLOOR: f32 = 0.2;

/// Maximum number of concept-embedding KNN hits considered as seeds.
pub const SEED_TOP_K: usize = 5;

/// How many typed-edge hops to expand from each seed concept. The neighbourhood
/// is traversed undirected (incoming + outgoing edges) so a seed reached via an
/// incoming edge — the canonical `Maria —[endangers]→ Q3` case — still
/// collects the source concept's braindumps.
pub const EXPAND_DEPTH: usize = 2;

/// Maximum braindumps returned by braindump-embedding KNN (backfill and the
/// no-seed fallback).
pub const BRAINDUMP_TOP_K: usize = 10;

/// The mode the retrieval pipeline ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalMode {
    SeedThenExpand,
    NoSeedFallback,
}

/// How a braindump entered the result set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BraindumpSource {
    /// Collected from the traversed subgraph (concept extraction provenance;
    /// edge provenance is a subset — a braindump asserting an edge always
    /// extracts both endpoints — so concept provenance captures the subgraph).
    Subgraph,
    /// Braindump-embedding KNN for strays the graph missed.
    Backfill,
    /// No-seed fallback: braindump-vector-direct (ADR-0004).
    VectorDirect,
}

/// A braindump in the retrieval result, with its rank score and origin.
#[derive(Debug, Clone, Serialize)]
pub struct RetrievedBraindump {
    pub id: i64,
    pub verbatim: String,
    pub cleaned: String,
    pub created_at: i64,
    pub score: f32,
    pub source: BraindumpSource,
}

/// A typed edge traversed during neighbourhood expansion — the citable structure
/// connecting the seed to the returned braindumps.
#[derive(Debug, Clone, Serialize)]
pub struct RetrievedEdge {
    pub source_concept_id: i64,
    pub source_concept_label: String,
    pub target_concept_id: i64,
    pub target_concept_label: String,
    pub edge_type: String,
}

/// The retrieval pipeline result.
#[derive(Debug, Clone, Serialize)]
pub struct RetrievalResult {
    pub braindumps: Vec<RetrievedBraindump>,
    pub paths: Vec<RetrievedEdge>,
    pub mode: RetrievalMode,
}

/// Run seed-then-expand retrieval (or the no-seed fallback) for a query.
///
/// Embeds the query (query task type, LLM), then delegates the full
/// seed→expand→collect→backfill pipeline to [`GraphRepo::retrieve`] (issue #47)
/// on a [`SqliteGraphRepo`]. The trait method takes the precomputed `query_vec`
/// and owns the pure-DB pipeline; the petgraph BFS is private to the adapter's
/// retrieval impl.
pub async fn retrieve(
    db: &Db,
    user_id: &str,
    llm: &dyn Llm,
    query: &str,
) -> Result<RetrievalResult> {
    let query_vec = llm.embed_query(query).await?;
    db.ensure_vec_tables(llm.dim())?;
    SqliteGraphRepo::new(db.clone())
        .retrieve(user_id, &query_vec)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::ingest_extraction;
    use crate::llm::{FakeLlm, Llm};

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeLlm::default().dim()).unwrap();
        db
    }

    fn fake_llm() -> FakeLlm {
        FakeLlm::default()
    }

    fn extraction(concepts: &[&str], edges: &[(&str, &str, &str)]) -> ExtractionResult {
        ExtractionResult {
            concepts: concepts
                .iter()
                .map(|l| ExtractedConcept {
                    label: l.to_string(),
                })
                .collect(),
            edges: edges
                .iter()
                .map(|(s, t, tg)| ExtractedEdge {
                    from_label: s.to_string(),
                    type_slug: t.to_string(),
                    to_label: tg.to_string(),
                })
                .collect(),
        }
    }

    async fn seed_braindump(db: &Db, _user_id: &str, text: &str) -> i64 {
        let b = insert_braindump(db, BOOTSTRAP_ADMIN_USER_ID, text, text)
            .await
            .unwrap();
        b.id
    }

    #[tokio::test]
    async fn seed_finds_braindumps_of_the_matched_concept() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "the q3 review went off the rails",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "the q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, "Q3 review")
            .await
            .unwrap();

        assert_eq!(result.mode, RetrievalMode::SeedThenExpand);
        let found = result
            .braindumps
            .iter()
            .find(|b| b.id == bd)
            .expect("seed concept's braindump returned");
        assert_eq!(found.source, BraindumpSource::Subgraph);
    }

    #[tokio::test]
    async fn expand_finds_braindump_connected_by_edge_but_not_containing_query_word() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "maria leaving tanks the timeline",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd,
            "maria leaving tanks the timeline",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        let result = retrieve(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, "Q3")
            .await
            .unwrap();

        assert_eq!(result.mode, RetrievalMode::SeedThenExpand);
        let found = result
            .braindumps
            .iter()
            .find(|b| b.id == bd)
            .expect("the graph-linked braindump is found via expansion");
        assert_eq!(found.source, BraindumpSource::Subgraph);
        assert!(
            !found.verbatim.to_lowercase().contains("q3"),
            "the found braindump must not lexically contain the query word"
        );
        assert!(
            result.paths.iter().any(|e| {
                e.source_concept_label == "Maria"
                    && e.target_concept_label == "Q3 launch"
                    && e.edge_type == "endangers"
            }),
            "traversed edge path present: {:?}",
            result.paths
        );
    }

    #[tokio::test]
    async fn backfill_finds_strays_the_graph_missed() {
        let db = test_db();
        let llm = fake_llm();

        let bd_graph = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "maria endangers the q3 launch",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_graph,
            "maria endangers the q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        let bd_stray =
            seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "q3 risk assessment notes").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_stray,
            "q3 risk assessment notes",
            extraction(&["Risk assessment"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(&db, BOOTSTRAP_ADMIN_USER_ID, &llm, "Q3")
            .await
            .unwrap();

        let stray = result
            .braindumps
            .iter()
            .find(|b| b.id == bd_stray)
            .expect("stray braindump found via backfill");
        assert_eq!(stray.source, BraindumpSource::Backfill);
        assert!(stray.score > 0.0);
    }

    #[tokio::test]
    async fn no_seed_fallback_retrieves_braindumps_vector_direct() {
        let db = test_db();
        let llm = fake_llm();

        let bd_reflective = seed_braindump(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            "feeling overwhelmed but my mind is full lately",
        )
        .await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_reflective,
            "feeling overwhelmed but my mind is full lately",
            extraction(&["Burnout"], &[]),
        )
        .await
        .unwrap();

        let bd_unrelated =
            seed_braindump(&db, BOOTSTRAP_ADMIN_USER_ID, "the q3 launch timeline").await;
        ingest_extraction(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            bd_unrelated,
            "the q3 launch timeline",
            extraction(&["Q3 launch"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
            &llm,
            "what is on my mind lately",
        )
        .await
        .unwrap();

        assert_eq!(result.mode, RetrievalMode::NoSeedFallback);
        assert!(result.paths.is_empty(), "no graph traversal in fallback");
        let found = result
            .braindumps
            .iter()
            .find(|b| b.id == bd_reflective)
            .expect("reflective braindump found vector-direct");
        assert_eq!(found.source, BraindumpSource::VectorDirect);
    }
}
