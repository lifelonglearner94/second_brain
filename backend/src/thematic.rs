//! The Thematic Read Model (issue #12, ADR-0008) — the third read surface
//! alongside Retrieval (ADR-0004) and Chat (ADR-0005/0006).
//!
//! A backend-owned projection of the knowledge graph's topology into clusters,
//! computed by Louvain community detection. The frontend renders this
//! projection; it never computes it (ADR-0008). Clusters are **ephemeral**: no
//! stable identity, no provenance, no persistence across sessions — they reflect
//! the graph's "now" and are re-computed on every read. Within one partition,
//! clusters carry throwaway session labels ("Group N for this session") so the
//! LLM can reference them in chat reasoning; the labels die with the partition.
//!
//! Louvain is a topology computation, not a rendering concern: it runs over the
//! undirected, weight-accumulated concept graph (each typed edge contributes
//! unit weight; multiple typed edges between the same pair accumulate). The
//! algorithm is non-deterministic by design (ADR-0008) — node visit order is
//! shuffled with a fresh RNG on every read, so partitions may differ across
//! sessions. Tests inject a seeded RNG for reproducibility.

use std::collections::HashMap;

use rand::rngs::{OsRng, StdRng};
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};
use serde::Serialize;

use crate::db::Db;
use crate::error::Result;

/// One cluster in the current partition: an ephemeral session label plus the
/// concept ids and labels it contains. The label is a throwaway "Group N for
/// this session" tag (ADR-0008) so the LLM can reference the cluster in chat
/// reasoning; it is NOT a stable id and dies with the partition.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Cluster {
    pub label: String,
    pub concept_ids: Vec<i64>,
    pub concept_labels: Vec<String>,
}

/// The current partition: the clusters Louvain found plus the concept count at
/// computation time (so the frontend can see the projection's coverage).
/// Non-deterministic across sessions by design (ADR-0008) — re-computed on every
/// read, never persisted.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Partition {
    pub clusters: Vec<Cluster>,
    pub concept_count: usize,
}

/// Compute the current partition: load the graph topology, run Louvain, assign
/// ephemeral session labels. Non-deterministic across reads (ADR-0008) — a
/// fresh `StdRng` seeded from the OS entropy seeds the node-visit shuffle every
/// call (`StdRng` is `Send`+`Sync`, so the future is `Send` for axum; the
/// `thread_rng` helper is `!Send` because it holds an `Rc`). Pure read: nothing
/// is persisted.
pub async fn partition(db: &Db) -> Result<Partition> {
    let mut rng = StdRng::from_rng(OsRng).expect("seeding StdRng from OS entropy");
    partition_with(db, &mut rng).await
}

/// Compute the partition with an injected RNG — the test seam that makes the
/// non-deterministic algorithm reproducible (ADR-0008's non-determinism is a
/// production property; tests need determinism).
pub async fn partition_with(db: &Db, rng: &mut impl Rng) -> Result<Partition> {
    let topology = load_topology(db).await?;
    let communities = louvain(&topology.graph, rng);
    Ok(label_partition(&topology, communities))
}

/// The graph topology loaded for clustering: each concept as a node (index =
/// position) plus the undirected, weight-accumulated edge graph Louvain runs
/// over.
struct Topology {
    /// `(concept_id, label)` per node index, ordered by concept id.
    concepts: Vec<(i64, String)>,
    graph: WeightedGraph,
}

/// An undirected weighted graph as an adjacency list. Each undirected edge
/// `(u, v, w)` appears in both `neighbors[u]` and `neighbors[v]`; a self-loop
/// `(u, u, w)` appears once in `neighbors[u]` and counts twice toward the
/// degree. `total_weight` is the sum of edge weights with each edge (including
/// self-loops) counted once, so `2 * total_weight == sum of degrees`.
#[derive(Debug, Clone, Default)]
struct WeightedGraph {
    neighbors: Vec<Vec<(usize, f64)>>,
    total_weight: f64,
}

impl WeightedGraph {
    fn n(&self) -> usize {
        self.neighbors.len()
    }

    /// Weighted degree of `i` (self-loops counted twice).
    fn degree(&self, i: usize) -> f64 {
        self.neighbors[i]
            .iter()
            .map(|&(j, w)| if j == i { 2.0 * w } else { w })
            .sum()
    }

    /// Each undirected edge once, as `(u, v, w)` with `u <= v` (self-loops as
    /// `(u, u, w)`). Used by the aggregation phase to rebuild the graph without
    /// double-counting.
    fn undirected_edges(&self) -> Vec<(usize, usize, f64)> {
        let mut edges = Vec::new();
        for (i, nbrs) in self.neighbors.iter().enumerate() {
            for &(j, w) in nbrs {
                if j >= i {
                    edges.push((i, j, w));
                }
            }
        }
        edges
    }

    /// Build from accumulated per-pair weights. `weights` keys are `(u, v)` with
    /// `u < v`; values are the accumulated edge weight between them.
    fn from_accumulated(n: usize, weights: &HashMap<(usize, usize), f64>) -> Self {
        let mut neighbors = vec![Vec::new(); n];
        let mut total_weight = 0.0;
        for (&(u, v), &w) in weights {
            if w <= 0.0 {
                continue;
            }
            if u == v {
                neighbors[u].push((u, w));
            } else {
                neighbors[u].push((v, w));
                neighbors[v].push((u, w));
            }
            total_weight += w;
        }
        WeightedGraph {
            neighbors,
            total_weight,
        }
    }
}

/// Load the graph topology for clustering: every concept (node) and every edge
/// (undirected, weight-accumulated). Each typed edge contributes unit weight;
/// multiple typed edges between the same concept pair (ADR-0002 contradictory
/// or reinforcing edges) accumulate into a stronger link. Self-edges
/// (`source == target`) are skipped — they carry no community signal. Edges
/// whose endpoints are not live concepts are skipped defensively.
async fn load_topology(db: &Db) -> Result<Topology> {
    db.run(|conn| {
        let mut concepts: Vec<(i64, String)> = Vec::new();
        {
            let mut stmt = conn.prepare("SELECT id, label FROM concepts ORDER BY id")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
            for row in rows {
                concepts.push(row?);
            }
        }
        let mut id_to_idx: HashMap<i64, usize> = HashMap::new();
        for (idx, (id, _)) in concepts.iter().enumerate() {
            id_to_idx.insert(*id, idx);
        }
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT source_concept_id, target_concept_id FROM edges ORDER BY id")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (s, t) = row?;
            if s == t {
                continue;
            }
            let (Some(&si), Some(&ti)) = (id_to_idx.get(&s), id_to_idx.get(&t)) else {
                continue;
            };
            let key = (si.min(ti), si.max(ti));
            *weights.entry(key).or_insert(0.0) += 1.0;
        }
        let graph = WeightedGraph::from_accumulated(concepts.len(), &weights);
        Ok(Topology { concepts, graph })
    })
    .await
}

/// Run Louvain community detection. Returns communities as vecs of original
/// node indices. Empty graph → no communities; edge-less graph → one singleton
/// per node.
fn louvain(graph: &WeightedGraph, rng: &mut impl Rng) -> Vec<Vec<usize>> {
    let n = graph.n();
    if n == 0 {
        return Vec::new();
    }
    let mut node_to_super: Vec<usize> = (0..n).collect();
    let mut level = graph.clone();
    const MAX_LEVELS: usize = 64;
    for _ in 0..MAX_LEVELS {
        let comm = local_moving(&level, rng);
        if community_count(&comm) == level.n() {
            break;
        }
        let aggregated = aggregate(&level, &comm);
        for super_slot in node_to_super.iter_mut() {
            let old_super = *super_slot;
            *super_slot = comm[old_super];
        }
        level = aggregated;
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (orig, &community) in node_to_super.iter().enumerate() {
        groups.entry(community).or_default().push(orig);
    }
    groups.into_values().collect()
}

/// Phase 1 of Louvain: local moving. Each node starts in its own community; in
/// random order, each node is removed from its community and moved to the
/// neighboring community that maximizes the modularity gain (only if it beats
/// staying). Passes repeat until a full pass moves nothing. Returns a
/// `node -> community` map renumbered to contiguous `0..k`.
fn local_moving(graph: &WeightedGraph, rng: &mut impl Rng) -> Vec<usize> {
    let n = graph.n();
    if n == 0 {
        return Vec::new();
    }
    let m = graph.total_weight;
    if m <= 0.0 {
        return (0..n).collect();
    }
    let two_m = 2.0 * m;
    let mut comm: Vec<usize> = (0..n).collect();
    let mut sigma_tot: Vec<f64> = (0..n).map(|i| graph.degree(i)).collect();
    let mut order: Vec<usize> = (0..n).collect();
    const MAX_PASSES: usize = 100;
    for _ in 0..MAX_PASSES {
        let mut moved = false;
        order.shuffle(rng);
        for &i in &order {
            let ci = comm[i];
            let ki = graph.degree(i);
            sigma_tot[ci] -= ki;
            let mut k_i_c: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in &graph.neighbors[i] {
                if j == i {
                    continue;
                }
                *k_i_c.entry(comm[j]).or_insert(0.0) += w;
            }
            let stay_kic = k_i_c.get(&ci).copied().unwrap_or(0.0);
            let mut best_comm = ci;
            let mut best_gain = gain(stay_kic, sigma_tot[ci], ki, two_m, m);
            for (&c, &kic) in &k_i_c {
                if c == ci {
                    continue;
                }
                let g = gain(kic, sigma_tot[c], ki, two_m, m);
                if g > best_gain + 1e-12 {
                    best_gain = g;
                    best_comm = c;
                }
            }
            comm[i] = best_comm;
            sigma_tot[best_comm] += ki;
            if best_comm != ci {
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    renumber(comm)
}

/// Modularity gain of moving isolated node `i` (degree `ki`) into community `C`
/// where `k_iC` is the weight of i's links into `C` and `sigma_tot_C` is the
/// sum of degrees in `C`. Derived: `ΔQ = k_iC / m − (sigma_tot_C · ki) / (2m²)`.
fn gain(k_i_c: f64, sigma_tot_c: f64, ki: f64, two_m: f64, m: f64) -> f64 {
    k_i_c / m - (sigma_tot_c * ki) / (two_m * m)
}

/// Renumber community ids to contiguous `0..k`, returning the renumbered
/// `node -> community` map.
fn renumber(comm: Vec<usize>) -> Vec<usize> {
    let mut mapping: HashMap<usize, usize> = HashMap::new();
    let mut next = 0;
    comm.iter()
        .map(|&c| {
            *mapping.entry(c).or_insert_with(|| {
                let id = next;
                next += 1;
                id
            })
        })
        .collect()
}

/// Number of distinct communities in a renumbered `node -> community` map (ids
/// are contiguous `0..k`, so the count is `max + 1`).
fn community_count(comm: &[usize]) -> usize {
    comm.iter().copied().max().unwrap_or(0) + 1
}

/// Phase 2 of Louvain: aggregate the graph so each community becomes one node.
/// Inter-community edge weights accumulate; intra-community edge weight becomes
/// a self-loop on the community's node.
fn aggregate(graph: &WeightedGraph, comm: &[usize]) -> WeightedGraph {
    let k = community_count(comm);
    let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
    for (i, j, w) in graph.undirected_edges() {
        let ci = comm[i];
        let cj = comm[j];
        let key = (ci.min(cj), ci.max(cj));
        *weights.entry(key).or_insert(0.0) += w;
    }
    WeightedGraph::from_accumulated(k, &weights)
}

/// Assign ephemeral session labels to the raw communities and build the public
/// [`Partition`]. Communities are ordered by size descending (then by smallest
/// concept id for a stable tie-break) so the largest cluster is "Group 1";
/// within a cluster, concepts are ordered by id. Every concept lands in exactly
/// one cluster (Louvain partitions the node set).
fn label_partition(topology: &Topology, mut communities: Vec<Vec<usize>>) -> Partition {
    communities.sort_by(|a, b| {
        b.len()
            .cmp(&a.len())
            .then_with(|| a.iter().copied().min().cmp(&b.iter().copied().min()))
    });
    let mut clusters = Vec::with_capacity(communities.len());
    for (i, comm) in communities.iter().enumerate() {
        let mut ids: Vec<i64> = comm.iter().map(|&idx| topology.concepts[idx].0).collect();
        ids.sort_unstable();
        let labels: Vec<String> = ids
            .iter()
            .map(|&id| {
                topology
                    .concepts
                    .iter()
                    .find(|(cid, _)| *cid == id)
                    .map(|(_, label)| label.clone())
                    .unwrap_or_default()
            })
            .collect();
        clusters.push(Cluster {
            label: format!("Group {} for this session", i + 1),
            concept_ids: ids,
            concept_labels: labels,
        });
    }
    Partition {
        clusters,
        concept_count: topology.concepts.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded_rng() -> StdRng {
        StdRng::seed_from_u64(0xC0DE_1234)
    }

    /// Two 3-cliques joined by a single weak bridge → Louvain must recover the
    /// two cliques as separate communities.
    fn two_cliques_graph() -> WeightedGraph {
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        // Clique A: {0,1,2}
        for pair in [(0, 1), (0, 2), (1, 2)] {
            weights.insert(pair, 1.0);
        }
        // Clique B: {3,4,5}
        for pair in [(3, 4), (3, 5), (4, 5)] {
            weights.insert(pair, 1.0);
        }
        // One weak bridge between the cliques.
        weights.insert((2, 3), 1.0);
        WeightedGraph::from_accumulated(6, &weights)
    }

    #[test]
    fn louvain_recovers_two_communities_on_two_cliques_joined_by_a_weak_bridge() {
        let graph = two_cliques_graph();
        let mut rng = seeded_rng();
        let mut communities = louvain(&graph, &mut rng);
        for c in &mut communities {
            c.sort_unstable();
        }
        communities.sort_unstable();
        assert_eq!(communities, vec![vec![0, 1, 2], vec![3, 4, 5]]);
    }

    #[test]
    fn louvain_merges_a_single_edge_into_one_community() {
        // A single edge is the densest possible 2-node graph: merging raises
        // modularity, so Louvain collapses the pair into one community.
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        weights.insert((0, 1), 1.0);
        let graph = WeightedGraph::from_accumulated(2, &weights);
        let mut rng = seeded_rng();
        let mut communities = louvain(&graph, &mut rng);
        for c in &mut communities {
            c.sort_unstable();
        }
        communities.sort_unstable();
        assert_eq!(communities, vec![vec![0, 1]]);
    }

    #[test]
    fn louvain_keeps_disconnected_nodes_as_singletons() {
        // No edges → no modularity gain from merging → every node its own
        // community (the edge-less graph is the degenerate "no structure" case).
        let graph = WeightedGraph::from_accumulated(3, &HashMap::new());
        let mut rng = seeded_rng();
        let mut communities = louvain(&graph, &mut rng);
        for c in &mut communities {
            c.sort_unstable();
        }
        communities.sort_unstable();
        assert_eq!(communities, vec![vec![0], vec![1], vec![2]]);
    }

    #[test]
    fn louvain_on_an_empty_graph_returns_no_communities() {
        let graph = WeightedGraph::from_accumulated(0, &HashMap::new());
        let mut rng = seeded_rng();
        assert!(louvain(&graph, &mut rng).is_empty());
    }

    #[test]
    fn louvain_accumulates_weight_from_parallel_edges() {
        // Two pairs where one pair has triple edge weight vs a single bridge:
        // {0,1} linked by three parallel edges, {2,3} by three, plus one weak
        // bridge 1-2. Accumulated weight must keep the dense pairs separate
        // (this is the ADR-0002 multi-typed-edge accretion signal Louvain sees).
        let mut weights: HashMap<(usize, usize), f64> = HashMap::new();
        *weights.entry((0, 1)).or_insert(0.0) += 3.0;
        *weights.entry((2, 3)).or_insert(0.0) += 3.0;
        weights.insert((1, 2), 1.0);
        let graph = WeightedGraph::from_accumulated(4, &weights);
        let mut rng = seeded_rng();
        let mut communities = louvain(&graph, &mut rng);
        for c in &mut communities {
            c.sort_unstable();
        }
        communities.sort_unstable();
        assert_eq!(communities, vec![vec![0, 1], vec![2, 3]]);
    }

    // --- DB-backed tests: load_topology + partition_with + label_partition ---

    use crate::braindump::insert_braindump;
    use crate::llm::{FakeLlm, Llm};
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::{concept_id_for_label, ingest_extraction};

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
        insert_braindump(db, text, text).await.unwrap().id
    }

    async fn ingest(db: &Db, text: &str, ext: ExtractionResult) {
        let bd = seed_braindump(db, text).await;
        ingest_extraction(db, &fake_llm(), bd, text, ext)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn load_topology_loads_concepts_as_nodes_and_edges_as_undirected_weights() {
        let db = test_db();
        ingest(
            &db,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;

        let topo = load_topology(&db).await.unwrap();
        assert_eq!(topo.concepts.len(), 2, "two concept nodes");
        assert_eq!(topo.graph.n(), 2);
        assert_eq!(topo.graph.total_weight, 1.0, "one undirected edge");
        let (a, b) = (0, 1);
        assert_eq!(topo.graph.degree(a), 1.0);
        assert_eq!(topo.graph.degree(b), 1.0);
    }

    #[tokio::test]
    async fn load_topology_accumulates_weight_across_multiple_typed_edges_between_a_pair() {
        // ADR-0002: two typed edges between the same pair (endangers + helps)
        // accrete as separate rows; Louvain sees them as one undirected link of
        // weight 2 — the multi-assertion density signal.
        let db = test_db();
        ingest(
            &db,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;
        ingest(
            &db,
            "maria helps q3",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await;

        let topo = load_topology(&db).await.unwrap();
        assert_eq!(topo.graph.n(), 2, "still two concept nodes");
        assert_eq!(
            topo.graph.total_weight, 2.0,
            "two typed edges accumulate into weight 2"
        );
        assert_eq!(topo.graph.degree(0), 2.0);
        assert_eq!(topo.graph.degree(1), 2.0);
    }

    #[tokio::test]
    async fn load_topology_skips_self_edges() {
        // A self-edge (source == target) carries no community signal and is
        // skipped — it must not inflate a node's degree or appear as a link.
        let db = test_db();
        ingest(
            &db,
            "maria endangers maria",
            extraction(&["Maria"], &[("Maria", "endangers", "Maria")]),
        )
        .await;

        let topo = load_topology(&db).await.unwrap();
        assert_eq!(topo.graph.n(), 1, "one concept node");
        assert_eq!(topo.graph.total_weight, 0.0, "self-edge skipped");
        assert_eq!(topo.graph.degree(0), 0.0);
    }

    #[tokio::test]
    async fn partition_assigns_group_n_for_this_session_labels_covering_every_concept() {
        // Two disjoint edges → two clusters. Every concept lands in exactly one
        // cluster; labels are the ADR-0008 ephemeral "Group N for this session"
        // format; the largest cluster is Group 1.
        let db = test_db();
        ingest(
            &db,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;
        ingest(
            &db,
            "alpha helps beta",
            extraction(&["Alpha", "Beta"], &[("Alpha", "helps", "Beta")]),
        )
        .await;

        let mut rng = seeded_rng();
        let partition = partition_with(&db, &mut rng).await.unwrap();

        assert_eq!(partition.concept_count, 4, "all four concepts projected");
        assert_eq!(partition.clusters.len(), 2, "two disjoint-edge clusters");
        let labels: Vec<&str> = partition
            .clusters
            .iter()
            .map(|c| c.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["Group 1 for this session", "Group 2 for this session"],
            "ADR-0008 ephemeral session labels, largest first"
        );
        // Every concept appears in exactly one cluster.
        let mut all_ids: Vec<i64> = partition
            .clusters
            .iter()
            .flat_map(|c| c.concept_ids.iter().copied())
            .collect();
        all_ids.sort_unstable();
        let maria = concept_id_for_label(&db, "Maria").await.unwrap().unwrap();
        let q3 = concept_id_for_label(&db, "Q3 launch")
            .await
            .unwrap()
            .unwrap();
        let alpha = concept_id_for_label(&db, "Alpha").await.unwrap().unwrap();
        let beta = concept_id_for_label(&db, "Beta").await.unwrap().unwrap();
        assert_eq!(all_ids, {
            let mut v = vec![maria, q3, alpha, beta];
            v.sort_unstable();
            v
        });
        // Each cluster's labels line up with its ids.
        for c in &partition.clusters {
            assert_eq!(
                c.concept_ids.len(),
                c.concept_labels.len(),
                "labels and ids paired"
            );
        }
    }

    #[tokio::test]
    async fn partition_on_an_empty_graph_has_no_clusters() {
        let db = test_db();
        let mut rng = seeded_rng();
        let partition = partition_with(&db, &mut rng).await.unwrap();
        assert_eq!(partition.concept_count, 0);
        assert!(partition.clusters.is_empty(), "no clusters on empty graph");
    }

    #[tokio::test]
    async fn partition_is_a_pure_read_no_persistence_across_calls() {
        // ADR-0008: clusters are ephemeral — computing a partition must not
        // persist anything. Assert no clusters table exists and the graph is
        // unchanged after repeated reads.
        let db = test_db();
        ingest(
            &db,
            "maria endangers q3",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await;
        let concepts_before = count_concepts(&db).await;
        let edges_before = count_edges(&db).await;

        let mut rng = seeded_rng();
        let _ = partition_with(&db, &mut rng).await.unwrap();
        let _ = partition_with(&db, &mut rng).await.unwrap();

        assert_eq!(
            count_concepts(&db).await,
            concepts_before,
            "no new concepts"
        );
        assert_eq!(count_edges(&db).await, edges_before, "no new edges");
        assert!(
            !clusters_table_exists(&db).await,
            "no clusters table — clusters are never persisted (ADR-0008)"
        );
    }

    async fn count_concepts(db: &Db) -> i64 {
        db.run(|conn| Ok(conn.query_row("SELECT count(*) FROM concepts", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    async fn count_edges(db: &Db) -> i64 {
        db.run(|conn| Ok(conn.query_row("SELECT count(*) FROM edges", [], |r| r.get(0))?))
            .await
            .unwrap()
    }

    async fn clusters_table_exists(db: &Db) -> bool {
        db.run(|conn| {
            let n: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='clusters'",
                [],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .await
        .unwrap()
    }
}
