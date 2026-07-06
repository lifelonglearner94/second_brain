//! The Global Topology Snapshot (issue #27) — the primary read surface for
//! visualization: a single payload returning the full graph topology — all
//! concepts, all typed edges with their projected current type (ADR-0003), and
//! the current Louvain partition IDs (ADR-0008). The frontend fetches this
//! wholesale on app load and caches it in IndexedDB for offline rendering; the
//! backend owns all graph computation, including the partition IDs (ADR-0008 —
//! the frontend never runs Louvain). This is the full read; the incremental
//! read is `GET /graph/delta` (issue #28).

use serde::Serialize;

use crate::db::Db;
use crate::error::Result;
use crate::graph::{Concept, EdgeProjection};
use crate::graph_repo::GraphRepo;
use crate::thematic;

/// The Global Topology Snapshot: every concept, every typed edge with its
/// projected current type, and the current Louvain partition assignment for
/// every concept — the one payload the frontend fetches to render the full
/// graph.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TopologySnapshot {
    pub concepts: Vec<Concept>,
    pub edges: Vec<EdgeProjection>,
    pub partitions: Vec<PartitionAssignment>,
}

/// One concept's assignment to its current Louvain community (ADR-0008). The
/// `partition_id` is an ephemeral session-scoped integer (no stable identity,
/// no persistence); concepts in the same community share an id, different
/// communities differ. Re-computed on every read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartitionAssignment {
    pub concept_id: i64,
    pub partition_id: u32,
}

/// Build the Global Topology Snapshot: all concepts, all typed edges with
/// their projected current type (ADR-0003), and the current Louvain partition
/// assignment for every concept (via [`thematic::partition`], ADR-0008).
/// Pure read; nothing is persisted.
///
/// Issue #45 routed the graph reads through the [`GraphRepo`] trait so the
/// snapshot builder depends on the interface, not the storage adapter. The
/// partition computation still touches `Db` directly (`thematic::partition`
/// builds the graph from a `Db::with_conn` closure — #47's scope to migrate); the
/// concept + edge reads go through `repo`.
pub async fn topology_snapshot(
    repo: &dyn GraphRepo,
    db: &Db,
    user_id: &str,
) -> Result<TopologySnapshot> {
    let concepts = repo.all_concepts(user_id).await?;
    let edges = repo.all_edges_with_current_type(user_id).await?;
    let partitions = partition_assignments(db, user_id).await?;
    Ok(TopologySnapshot {
        concepts,
        edges,
        partitions,
    })
}

/// Map the current Louvain partition (ADR-0008) into a flat per-concept
/// assignment list. The cluster index (0-based, in [`thematic::Partition`]'s
/// size-descending order) is the ephemeral `partition_id`; concepts in the same
/// cluster share it, different clusters differ.
fn partition_assignments_from(partition: &thematic::Partition) -> Vec<PartitionAssignment> {
    let mut assignments = Vec::new();
    for (idx, cluster) in partition.clusters.iter().enumerate() {
        for &concept_id in &cluster.concept_ids {
            assignments.push(PartitionAssignment {
                concept_id,
                partition_id: idx as u32,
            });
        }
    }
    assignments
}

async fn partition_assignments(db: &Db, user_id: &str) -> Result<Vec<PartitionAssignment>> {
    let partition = thematic::partition(db, user_id).await?;
    Ok(partition_assignments_from(&partition))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::db::BOOTSTRAP_ADMIN_USER_ID;
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::{concept_id_for_label, find_edge, ingest_extraction};
    use crate::graph_repo::SqliteGraphRepo;
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
        insert_braindump(db, BOOTSTRAP_ADMIN_USER_ID, text, text)
            .await
            .unwrap()
            .id
    }

    async fn ingest(db: &Db, text: &str, ext: ExtractionResult) {
        let bd = seed_braindump(db, text).await;
        ingest_extraction(db, BOOTSTRAP_ADMIN_USER_ID, &fake_llm(), bd, text, ext)
            .await
            .unwrap();
    }

    async fn label_id(db: &Db, label: &str) -> i64 {
        concept_id_for_label(db, BOOTSTRAP_ADMIN_USER_ID, label)
            .await
            .unwrap()
            .expect("concept exists")
    }

    fn partition_id_of(snap: &TopologySnapshot, concept_id: i64) -> u32 {
        snap.partitions
            .iter()
            .find(|p| p.concept_id == concept_id)
            .map(|p| p.partition_id)
            .expect("concept has a partition assignment")
    }

    /// Append a retag entry (seq_index = max+1) to an edge's type history —
    /// simulates the async ontology refactor (ADR-0003) without standing up
    /// the governance pipeline.
    async fn append_retag(db: &Db, edge_id: i64, type_slug: &str) {
        let type_slug = type_slug.to_string();
        db.with_conn(move |conn| {
            let next_seq: i64 = conn.query_row(
                "SELECT COALESCE(MAX(seq_index), -1) + 1 FROM edge_type_history WHERE edge_id = ?1",
                params![edge_id],
                |r| r.get(0),
            )?;
            let now = crate::db::now_seconds();
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![edge_id, next_seq, type_slug, now],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn topology_snapshot_returns_all_concepts_edges_with_current_type_and_partitions() {
        // Two disjoint edges → two Louvain communities. The snapshot carries
        // all four concepts, both edges with their projected current type
        // (== original for a fresh edge), and a partition assignment mapping
        // every concept to its community (same-community concepts share an id,
        // different communities differ).
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

        let snap = topology_snapshot(
            &SqliteGraphRepo::new(db.clone()),
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
        )
        .await
        .unwrap();
        assert_eq!(snap.concepts.len(), 4, "all four concepts");
        assert_eq!(snap.edges.len(), 2, "both edges");
        for e in &snap.edges {
            assert_eq!(
                e.current_type, e.original_type,
                "fresh edge: current type == original (projected from history)"
            );
        }
        assert_eq!(
            snap.partitions.len(),
            4,
            "every concept has a partition assignment"
        );
        let maria = label_id(&db, "Maria").await;
        let q3 = label_id(&db, "Q3 launch").await;
        let alpha = label_id(&db, "Alpha").await;
        let beta = label_id(&db, "Beta").await;
        assert_eq!(
            partition_id_of(&snap, maria),
            partition_id_of(&snap, q3),
            "Maria and Q3 in the same community"
        );
        assert_eq!(
            partition_id_of(&snap, alpha),
            partition_id_of(&snap, beta),
            "Alpha and Beta in the same community"
        );
        assert_ne!(
            partition_id_of(&snap, maria),
            partition_id_of(&snap, alpha),
            "the two communities have distinct partition ids"
        );
    }

    #[tokio::test]
    async fn topology_snapshot_projects_current_type_from_type_history_after_retag() {
        // ADR-0003: the current type is the projection of the last
        // type_history entry, not a stored field. After a refactor appends
        // "supports" to a "helps" edge's history, the snapshot reports
        // current_type = "supports" while original_type stays "helps".
        let db = test_db();
        ingest(
            &db,
            "maria helps q3",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await;
        let maria = label_id(&db, "Maria").await;
        let q3 = label_id(&db, "Q3 launch").await;
        let edge = find_edge(&db, BOOTSTRAP_ADMIN_USER_ID, maria, "helps", q3)
            .await
            .unwrap()
            .expect("edge created");
        append_retag(&db, edge.id, "supports").await;

        let snap = topology_snapshot(
            &SqliteGraphRepo::new(db.clone()),
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
        )
        .await
        .unwrap();
        let e = snap
            .edges
            .iter()
            .find(|e| e.id == edge.id)
            .expect("edge in snapshot");
        assert_eq!(e.original_type, "helps", "original assertion immutable");
        assert_eq!(
            e.current_type, "supports",
            "current type projected from history"
        );
        assert_ne!(
            e.current_type, e.original_type,
            "retag changed the projected current type"
        );
    }

    #[tokio::test]
    async fn topology_snapshot_on_empty_graph_returns_empty() {
        let db = test_db();
        let snap = topology_snapshot(
            &SqliteGraphRepo::new(db.clone()),
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
        )
        .await
        .unwrap();
        assert!(snap.concepts.is_empty(), "no concepts");
        assert!(snap.edges.is_empty(), "no edges");
        assert!(
            snap.partitions.is_empty(),
            "no concepts → no partition assignments"
        );
    }

    #[tokio::test]
    async fn topology_snapshot_assigns_a_partition_id_to_every_concept() {
        // Every concept in the snapshot has exactly one partition assignment;
        // the set of assigned concept ids equals the set of concept ids. The
        // lonely concept (no edges) is its own singleton community, distinct
        // from the Maria–Q3 community.
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
        ingest(&db, "a lonely concept", extraction(&["Lonely"], &[])).await;

        let snap = topology_snapshot(
            &SqliteGraphRepo::new(db.clone()),
            &db,
            BOOTSTRAP_ADMIN_USER_ID,
        )
        .await
        .unwrap();
        let mut concept_ids: Vec<i64> = snap.concepts.iter().map(|c| c.id).collect();
        concept_ids.sort_unstable();
        let mut assigned: Vec<i64> = snap.partitions.iter().map(|p| p.concept_id).collect();
        assigned.sort_unstable();
        assert_eq!(concept_ids, assigned, "every concept assigned, no extras");
        let lonely = label_id(&db, "Lonely").await;
        let maria = label_id(&db, "Maria").await;
        assert_ne!(
            partition_id_of(&snap, lonely),
            partition_id_of(&snap, maria),
            "lonely concept is its own community, distinct from Maria's"
        );
    }
}
