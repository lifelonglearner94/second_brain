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

use std::collections::{HashMap, HashSet, VecDeque};

use petgraph::stable_graph::{NodeIndex, StableUnGraph};
use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::db::Db;
use crate::error::Result;
use crate::graph::vec_to_blob;
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
/// Embeds the query (query task type), seeds via concept-embedding KNN, expands
/// the typed-edge neighbourhood, collects subgraph braindumps, backfills with
/// braindump-embedding KNN, and returns ranked braindumps plus the traversed
/// edge paths. When no concept seed clears [`SEED_SIMILARITY_FLOOR`], retrieval
/// falls back to braindump-vector-direct (ADR-0004 no-seed fallback).
pub async fn retrieve(
    db: &Db,
    llm: &dyn Llm,
    query: &str,
) -> Result<RetrievalResult> {
    let query_vec = llm.embed_query(query).await?;
    let dim = llm.dim();
    db.run(move |conn| {
        crate::db::ensure_vec_tables(conn, dim)?;
        retrieve_conn(conn, &query_vec)
    })
    .await
}

fn retrieve_conn(conn: &rusqlite::Connection, query_vec: &[f32]) -> Result<RetrievalResult> {
    let candidates = knn_concepts(conn, query_vec, SEED_TOP_K)?;
    let seeds: Vec<(i64, f32)> = candidates
        .into_iter()
        .filter(|(_, sim)| *sim >= SEED_SIMILARITY_FLOOR)
        .collect();

    if seeds.is_empty() {
        return no_seed_fallback(conn, query_vec);
    }

    let (concept_hops, traversed_edges) = expand(conn, &seeds)?;
    let subgraph = collect_subgraph_braindumps(conn, &concept_hops)?;
    let backfill = backfill_braindumps(conn, query_vec, &subgraph)?;

    let mut all = subgraph;
    all.extend(backfill);
    all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(RetrievalResult {
        braindumps: all,
        paths: traversed_edges,
        mode: RetrievalMode::SeedThenExpand,
    })
}

/// No-seed fallback (ADR-0004): the query had no concept anchor, so vectors
/// become primary — braindump-embedding KNN, vector-direct.
fn no_seed_fallback(conn: &rusqlite::Connection, query_vec: &[f32]) -> Result<RetrievalResult> {
    let hits = knn_braindumps(conn, query_vec, BRAINDUMP_TOP_K)?;
    let mut braindumps = Vec::new();
    for (bd_id, sim) in hits {
        if let Some(b) = load_braindump_row(conn, bd_id)? {
            braindumps.push(RetrievedBraindump {
                id: b.id,
                verbatim: b.verbatim,
                cleaned: b.cleaned,
                created_at: b.created_at,
                score: sim,
                source: BraindumpSource::VectorDirect,
            });
        }
    }
    Ok(RetrievalResult {
        braindumps,
        paths: Vec::new(),
        mode: RetrievalMode::NoSeedFallback,
    })
}

struct EdgeInfo {
    edge_type: String,
    source_concept_id: i64,
    target_concept_id: i64,
}

/// Build the typed-edge graph and BFS from the seed concepts up to
/// [`EXPAND_DEPTH`] hops (undirected: incoming + outgoing). Returns each visited
/// concept's minimum hop distance from a seed, and the edges in the traversed
/// subgraph.
fn expand(
    conn: &rusqlite::Connection,
    seeds: &[(i64, f32)],
) -> Result<(HashMap<i64, usize>, Vec<RetrievedEdge>)> {
    let mut concept_labels: HashMap<i64, String> = HashMap::new();
    {
        let mut stmt = conn.prepare("SELECT id, label FROM concepts")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (id, label) = row?;
            concept_labels.insert(id, label);
        }
    }

    let mut graph: StableUnGraph<i64, EdgeInfo> = StableUnGraph::default();
    let mut node_index: HashMap<i64, NodeIndex> = HashMap::new();
    for &cid in concept_labels.keys() {
        let idx = graph.add_node(cid);
        node_index.insert(cid, idx);
    }

    let mut stmt = conn.prepare(
        "SELECT e.source_concept_id, e.target_concept_id,
                (SELECT type_slug FROM edge_type_history
                 WHERE edge_id = e.id ORDER BY seq_index DESC LIMIT 1) AS current_type
         FROM edges e",
    )?;
    let edge_rows = stmt.query_map([], |r| {
        Ok(EdgeInfo {
            source_concept_id: r.get(0)?,
            target_concept_id: r.get(1)?,
            edge_type: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        })
    })?;
    for row in edge_rows {
        let info = row?;
        if let (Some(&s), Some(&t)) = (
            node_index.get(&info.source_concept_id),
            node_index.get(&info.target_concept_id),
        ) {
            graph.add_edge(s, t, info);
        }
    }

    let mut visited: HashSet<NodeIndex> = HashSet::new();
    let mut concept_hops: HashMap<i64, usize> = HashMap::new();
    let mut queue: VecDeque<(NodeIndex, usize)> = VecDeque::new();
    for &(seed_cid, _) in seeds {
        if let Some(&idx) = node_index.get(&seed_cid) {
            if visited.insert(idx) {
                concept_hops.insert(seed_cid, 0);
                queue.push_back((idx, 0));
            }
        }
    }
    while let Some((node, depth)) = queue.pop_front() {
        if depth >= EXPAND_DEPTH {
            continue;
        }
        for neighbor in graph.neighbors(node) {
            if visited.insert(neighbor) {
                if let Some(&cid) = graph.node_weight(neighbor) {
                    concept_hops.entry(cid).or_insert(depth + 1);
                }
                queue.push_back((neighbor, depth + 1));
            }
        }
    }

    let traversed: Vec<RetrievedEdge> = graph
        .edge_indices()
        .filter_map(|e| {
            let (s, t) = graph.edge_endpoints(e)?;
            if visited.contains(&s) && visited.contains(&t) {
                let info = graph.edge_weight(e)?;
                Some(RetrievedEdge {
                    source_concept_id: info.source_concept_id,
                    source_concept_label: concept_labels
                        .get(&info.source_concept_id)
                        .cloned()
                        .unwrap_or_default(),
                    target_concept_id: info.target_concept_id,
                    target_concept_label: concept_labels
                        .get(&info.target_concept_id)
                        .cloned()
                        .unwrap_or_default(),
                    edge_type: info.edge_type.clone(),
                })
            } else {
                None
            }
        })
        .collect();

    Ok((concept_hops, traversed))
}

/// Collect braindumps from the traversed subgraph: each visited concept's
/// extraction provenance (ADR-0010) plus each traversed edge's asserted_by
/// list (ADR-0002). Score decays with hop distance from the nearest seed.
fn collect_subgraph_braindumps(
    conn: &rusqlite::Connection,
    concept_hops: &HashMap<i64, usize>,
) -> Result<Vec<RetrievedBraindump>> {
    let mut best: HashMap<i64, f32> = HashMap::new();
    for (cid, hops) in concept_hops {
        let score = 1.0 / (1.0 + *hops as f32);
        let mut stmt =
            conn.prepare("SELECT braindump_id FROM concept_provenance WHERE concept_id = ?1")?;
        let ids = stmt.query_map(params![cid], |r| r.get::<_, i64>(0))?;
        for id in ids {
            let bd_id = id?;
            let entry = best.entry(bd_id).or_insert(0.0);
            if score > *entry {
                *entry = score;
            }
        }
    }

    let mut result = Vec::new();
    for (bd_id, score) in &best {
        if let Some(b) = load_braindump_row(conn, *bd_id)? {
            result.push(RetrievedBraindump {
                id: b.id,
                verbatim: b.verbatim,
                cleaned: b.cleaned,
                created_at: b.created_at,
                score: *score,
                source: BraindumpSource::Subgraph,
            });
        }
    }
    Ok(result)
}

/// Braindump-embedding KNN backfill for strays the graph missed (ADR-0004).
/// Returns braindumps not already in the subgraph set, scored by similarity.
fn backfill_braindumps(
    conn: &rusqlite::Connection,
    query_vec: &[f32],
    subgraph: &[RetrievedBraindump],
) -> Result<Vec<RetrievedBraindump>> {
    let already: HashSet<i64> = subgraph.iter().map(|b| b.id).collect();
    let hits = knn_braindumps(conn, query_vec, BRAINDUMP_TOP_K)?;
    let mut backfill = Vec::new();
    for (bd_id, sim) in hits {
        if already.contains(&bd_id) {
            continue;
        }
        if let Some(b) = load_braindump_row(conn, bd_id)? {
            backfill.push(RetrievedBraindump {
                id: b.id,
                verbatim: b.verbatim,
                cleaned: b.cleaned,
                created_at: b.created_at,
                score: sim,
                source: BraindumpSource::Backfill,
            });
        }
    }
    Ok(backfill)
}

struct BraindumpRow {
    id: i64,
    verbatim: String,
    cleaned: String,
    created_at: i64,
}

fn load_braindump_row(conn: &rusqlite::Connection, id: i64) -> Result<Option<BraindumpRow>> {
    let row = conn
        .query_row(
            "SELECT id, verbatim, cleaned, created_at FROM braindumps WHERE id = ?1",
            params![id],
            |r| {
                Ok(BraindumpRow {
                    id: r.get(0)?,
                    verbatim: r.get(1)?,
                    cleaned: r.get(2)?,
                    created_at: r.get(3)?,
                })
            },
        )
        .optional()?;
    Ok(row)
}

/// sqlite-vec KNN: top-K concepts by cosine similarity to the query vector.
/// similarity = 1 − distance (cosine metric on the vec0 table).
fn knn_concepts(
    conn: &rusqlite::Connection,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<(i64, f32)>> {
    let blob = vec_to_blob(query_vec);
    let mut stmt = conn.prepare(
        "SELECT concept_id, distance FROM concept_embeddings
         WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![blob, limit as i64], |r| {
        Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// sqlite-vec KNN: top-K braindumps by cosine similarity to the query vector.
fn knn_braindumps(
    conn: &rusqlite::Connection,
    query_vec: &[f32],
    limit: usize,
) -> Result<Vec<(i64, f32)>> {
    let blob = vec_to_blob(query_vec);
    let mut stmt = conn.prepare(
        "SELECT braindump_id, distance FROM braindump_embeddings
         WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![blob, limit as i64], |r| {
        Ok((r.get::<_, i64>(0)?, 1.0 - r.get::<_, f64>(1)? as f32))
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::llm::{FakeLlm, Llm};
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::ingest_extraction;

    fn test_db() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.ensure_vec_tables(FakeLlm::default().dim())
            .unwrap();
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

    async fn seed_braindump(db: &Db, text: &str) -> i64 {
        let b = insert_braindump(db, text, text).await.unwrap();
        b.id
    }

    #[tokio::test]
    async fn seed_finds_braindumps_of_the_matched_concept() {
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "the q3 review went off the rails").await;
        ingest_extraction(
            &db,
            &llm,
            bd,
            "the q3 review went off the rails",
            extraction(&["Q3 review"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(&db, &llm, "Q3 review").await.unwrap();

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
        // ADR-0004 canonical case: a braindump graph-linked
        // `Maria —[endangers]→ Q3 launch` but never containing the word "Q3" is
        // found by seeding on "Q3 launch" and traversing the incoming edge to
        // Maria.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria leaving tanks the timeline").await;
        ingest_extraction(
            &db,
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

        let result = retrieve(&db, &llm, "Q3").await.unwrap();

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
        // The traversed edge path is returned as citable structure.
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
        // A braindump whose concept does not seed and is not graph-connected
        // to the seed, but whose text matches the query — found by
        // braindump-embedding KNN backfill (ADR-0004).
        let db = test_db();
        let llm = fake_llm();

        let bd_graph = seed_braindump(&db, "maria endangers the q3 launch").await;
        ingest_extraction(
            &db,
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

        let bd_stray = seed_braindump(&db, "q3 risk assessment notes").await;
        ingest_extraction(
            &db,
            &llm,
            bd_stray,
            "q3 risk assessment notes",
            extraction(&["Risk assessment"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(&db, &llm, "Q3").await.unwrap();

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
        // ADR-0004 no-seed fallback: an unanchored query with no concept
        // anchor cannot seed and cannot expand; it falls back to
        // braindump-vector-direct — the one place vectors become primary.
        let db = test_db();
        let llm = fake_llm();

        let bd_reflective =
            seed_braindump(&db, "feeling overwhelmed but my mind is full lately").await;
        ingest_extraction(
            &db,
            &llm,
            bd_reflective,
            "feeling overwhelmed but my mind is full lately",
            extraction(&["Burnout"], &[]),
        )
        .await
        .unwrap();

        let bd_unrelated = seed_braindump(&db, "the q3 launch timeline").await;
        ingest_extraction(
            &db,
            &llm,
            bd_unrelated,
            "the q3 launch timeline",
            extraction(&["Q3 launch"], &[]),
        )
        .await
        .unwrap();

        let result = retrieve(&db, &llm, "what is on my mind lately")
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
