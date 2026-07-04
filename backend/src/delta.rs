//! Delta sync — the incremental read surface for pull-on-focus reconciliation
//! (issue #28).
//!
//! `graph_delta(db, since)` returns the changes since the client's cursor:
//! **additions** (new concepts/edges from ingests), **deletions** (concepts/edges
//! vanished via the braindump-deletion provenance cascade, ADR-0007/0010 —
//! recorded as tombstones by [`crate::graph::retract_extraction`]), and
//! **retags** (edges whose projected current type changed via the async
//! ontology refactor appending to `edge_type_history`, ADR-0003). A fresh
//! cursor timestamp is returned for the client's next pull.
//!
//! The backend stays stateless: the timestamp is the client's cursor, no
//! server-held session, no push channel. Single-user makes brief staleness
//! between focus events acceptable.
//!
//! Boundary semantics: changes are filtered by `created_at > since` (strict), so
//! a change at exactly the cursor timestamp is not re-reported on the next pull.
//! The returned cursor is `now_seconds()` at query time, which is `>=` every
//! returned row's timestamp, so nothing is missed across pulls — at worst a
//! same-second change lands in the next pull rather than this one (brief
//! staleness, accepted by the design).

use rusqlite::params;
use serde::Serialize;

use crate::db::{now_seconds, Db};
use crate::error::Result;
use crate::graph::{current_type_subquery, Concept};

/// The delta-sync response: every change since the client's cursor, plus a
/// fresh cursor for the next pull.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GraphDelta {
    /// Fresh cursor: the client passes this as `since` on its next pull.
    pub cursor: i64,
    /// Concepts created (first extracted) since the cursor.
    pub added_concepts: Vec<Concept>,
    /// Edges created (first asserted) since the cursor, with their projected
    /// current type so the client renders the right label from the start.
    pub added_edges: Vec<DeltaEdge>,
    /// Concept ids that vanished via the deletion cascade since the cursor
    /// (tombstoned in `graph_tombstones`). The client removes these nodes.
    pub deleted_concept_ids: Vec<i64>,
    /// Edge ids that vanished via the deletion cascade since the cursor.
    pub deleted_edge_ids: Vec<i64>,
    /// Pre-existing edges whose projected current type changed (a refactor
    /// appended to `edge_type_history`) since the cursor. Edges created after
    /// the cursor are excluded — they arrive as additions carrying their
    /// current type, so a redundant retag would double-report.
    pub retagged_edges: Vec<RetaggedEdge>,
}

/// An added edge with its projected current type (the last `edge_type_history`
/// entry — ADR-0003). For a freshly-created edge the current type equals the
/// original; including it keeps the addition self-describing even if a refactor
/// landed between creation and this pull.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeltaEdge {
    pub id: i64,
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub original_type: String,
    pub current_type: String,
    pub created_at: i64,
}

/// An edge retagged since the cursor: the client updates its type label to
/// `current_type` (the projected last entry of the append-only history).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RetaggedEdge {
    pub id: i64,
    pub source_concept_id: i64,
    pub target_concept_id: i64,
    pub original_type: String,
    pub current_type: String,
}

/// Compute the graph delta since `since`. Reads additions from
/// `concepts`/`edges` (filtered by `created_at`), deletions from
/// `graph_tombstones`, and retags from `edge_type_history` (entries with
/// `seq_index > 0`). The cursor is `now_seconds()` at query time.
pub async fn graph_delta(db: &Db, since: i64) -> Result<GraphDelta> {
    db.run(move |conn| {
        let added_concepts = added_concepts_since(conn, since)?;
        let added_edges = added_edges_since(conn, since)?;
        let deleted_concept_ids = tombstoned_since(conn, "concept", since)?;
        let deleted_edge_ids = tombstoned_since(conn, "edge", since)?;
        let retagged_edges = retagged_edges_since(conn, since)?;
        let cursor = now_seconds();
        Ok(GraphDelta {
            cursor,
            added_concepts,
            added_edges,
            deleted_concept_ids,
            deleted_edge_ids,
            retagged_edges,
        })
    })
    .await
}

fn added_concepts_since(conn: &rusqlite::Connection, since: i64) -> Result<Vec<Concept>> {
    let mut stmt = conn.prepare(
        "SELECT id, label, created_at FROM concepts
         WHERE created_at > ?1 ORDER BY id",
    )?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(Concept {
                id: r.get(0)?,
                label: r.get(1)?,
                created_at: r.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

fn added_edges_since(conn: &rusqlite::Connection, since: i64) -> Result<Vec<DeltaEdge>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id, e.source_concept_id, e.target_concept_id, e.original_type,
                e.created_at, ({}) AS current_type
         FROM edges e WHERE e.created_at > ?1 ORDER BY e.id",
        current_type_subquery()
    ))?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(DeltaEdge {
                id: r.get(0)?,
                source_concept_id: r.get(1)?,
                target_concept_id: r.get(2)?,
                original_type: r.get(3)?,
                created_at: r.get(4)?,
                current_type: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Concept/edge ids tombstoned (vanished via the deletion cascade) since the
/// cursor. `kind` is 'concept' or 'edge'.
fn tombstoned_since(conn: &rusqlite::Connection, kind: &str, since: i64) -> Result<Vec<i64>> {
    let mut stmt = conn.prepare(
        "SELECT entity_id FROM graph_tombstones
         WHERE kind = ?1 AND created_at > ?2 ORDER BY entity_id",
    )?;
    let ids = stmt
        .query_map(params![kind, since], |r| r.get::<_, i64>(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(ids)
}

/// Edges whose projected current type changed since the cursor: a refactor
/// appended a `seq_index > 0` entry to `edge_type_history` after the cursor.
/// Edges created after the cursor are excluded — they arrive as additions
/// carrying their current type, so including them here would double-report.
/// `current_type` is the projection of the last history entry (ADR-0003).
fn retagged_edges_since(conn: &rusqlite::Connection, since: i64) -> Result<Vec<RetaggedEdge>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT e.id, e.source_concept_id, e.target_concept_id, e.original_type,
                ({}) AS current_type
         FROM edges e
         WHERE e.created_at <= ?1 AND EXISTS (
             SELECT 1 FROM edge_type_history eth
             WHERE eth.edge_id = e.id AND eth.seq_index > 0 AND eth.created_at > ?1)
         ORDER BY e.id",
        current_type_subquery()
    ))?;
    let rows = stmt
        .query_map(params![since], |r| {
            Ok(RetaggedEdge {
                id: r.get(0)?,
                source_concept_id: r.get(1)?,
                target_concept_id: r.get(2)?,
                original_type: r.get(3)?,
                current_type: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::braindump::insert_braindump;
    use crate::llm::{FakeLlm, Llm};
    use crate::extractor::{ExtractedConcept, ExtractedEdge, ExtractionResult};
    use crate::graph::{concept_id_for_label, delete_braindump, find_edge, ingest_extraction};

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

    async fn db_concept_id_for_label(db: &Db, label: &str) -> i64 {
        concept_id_for_label(db, label).await.unwrap().unwrap_or(0)
    }

    /// Stamp every graph row's `created_at` to a fixed value so delta-filtering
    /// on a cursor between the stamp and a later retag is deterministic (the
    /// real `now_seconds()` has second-level granularity, which would make
    /// same-second boundary tests flaky).
    async fn backdate_graph(db: &Db, ts: i64) {
        db.run(move |conn| {
            conn.execute("UPDATE concepts SET created_at = ?1", params![ts])?;
            conn.execute("UPDATE edges SET created_at = ?1", params![ts])?;
            conn.execute("UPDATE edge_type_history SET created_at = ?1", params![ts])?;
            Ok(())
        })
        .await
        .unwrap();
    }

    /// Append a retag entry (seq_index = max+1) to an edge's type history at a
    /// fixed timestamp — simulates the async refactor (ADR-0003) without
    /// standing up the governance pipeline.
    async fn append_retag(db: &Db, edge_id: i64, type_slug: &str, ts: i64) {
        let type_slug = type_slug.to_string();
        db.run(move |conn| {
            let next_seq: i64 = conn.query_row(
                "SELECT COALESCE(MAX(seq_index), -1) + 1 FROM edge_type_history WHERE edge_id = ?1",
                params![edge_id],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT INTO edge_type_history (edge_id, seq_index, type_slug, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![edge_id, next_seq, type_slug, ts],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn delta_since_zero_returns_all_additions() {
        // First-sync cursor (since=0): every existing concept/edge is an
        // addition (all real timestamps > 0). No deletions, no retags.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            &llm,
            bd,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();

        let delta = graph_delta(&db, 0).await.unwrap();
        assert_eq!(delta.added_concepts.len(), 2, "both concepts added");
        assert!(delta.added_concepts.iter().any(|c| c.label == "Maria"));
        assert!(delta.added_concepts.iter().any(|c| c.label == "Q3 launch"));
        assert_eq!(delta.added_edges.len(), 1, "one edge added");
        let added = &delta.added_edges[0];
        assert_eq!(added.original_type, "endangers");
        assert_eq!(
            added.current_type, "endangers",
            "a freshly-created edge's current type equals its original"
        );
        assert!(
            delta.deleted_concept_ids.is_empty(),
            "no deletions on first sync"
        );
        assert!(delta.deleted_edge_ids.is_empty());
        assert!(delta.retagged_edges.is_empty(), "no retags on first sync");
        assert!(delta.cursor > 0, "cursor is fresh");
    }

    #[tokio::test]
    async fn delta_returns_deletions_for_vanished_concepts_and_edges() {
        // Ingest then delete a braindump: the vanished concept and edge are
        // tombstoned and reported as deletions (ADR-0007/0010 cascade).
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria endangers q3 launch").await;
        ingest_extraction(
            &db,
            &llm,
            bd,
            "maria endangers q3 launch",
            extraction(
                &["Maria", "Q3 launch"],
                &[("Maria", "endangers", "Q3 launch")],
            ),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, "Maria").await;
        let q3 = db_concept_id_for_label(&db, "Q3 launch").await;
        let edge = find_edge(&db, maria, "endangers", q3)
            .await
            .unwrap()
            .expect("edge created");

        delete_braindump(&db, bd).await.unwrap();

        let delta = graph_delta(&db, 0).await.unwrap();
        assert!(
            delta.deleted_concept_ids.contains(&maria),
            "vanished concept reported as deletion: {:?}",
            delta.deleted_concept_ids
        );
        assert!(
            delta.deleted_edge_ids.contains(&edge.id),
            "vanished edge reported as deletion: {:?}",
            delta.deleted_edge_ids
        );
    }

    #[tokio::test]
    async fn delta_returns_retag_for_edge_whose_current_type_changed() {
        // An edge created before the cursor and retagged after it: reported as
        // a retag with the projected current type (ADR-0003). Controlled
        // timestamps so the boundary is deterministic.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria helps q3 launch").await;
        ingest_extraction(
            &db,
            &llm,
            bd,
            "maria helps q3 launch",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, "Maria").await;
        let q3 = db_concept_id_for_label(&db, "Q3 launch").await;
        let edge = find_edge(&db, maria, "helps", q3)
            .await
            .unwrap()
            .expect("edge created");

        // Backdate the whole graph to ts=1000, then query with cursor=1500.
        backdate_graph(&db, 1000).await;
        let since = 1500;
        // Simulate a refactor retag at ts=2000 (after the cursor).
        append_retag(&db, edge.id, "supports", 2000).await;

        let delta = graph_delta(&db, since).await.unwrap();
        assert!(
            delta.added_concepts.is_empty(),
            "concepts created before cursor are not additions"
        );
        assert!(
            delta.added_edges.is_empty(),
            "edge created before cursor is not an addition"
        );
        assert_eq!(delta.retagged_edges.len(), 1, "exactly one retag");
        let retag = &delta.retagged_edges[0];
        assert_eq!(retag.id, edge.id);
        assert_eq!(retag.source_concept_id, maria);
        assert_eq!(retag.target_concept_id, q3);
        assert_eq!(retag.original_type, "helps", "original assertion preserved");
        assert_eq!(
            retag.current_type, "supports",
            "current type is the projected retag"
        );
    }

    #[tokio::test]
    async fn delta_returns_empty_when_nothing_changed_since_cursor() {
        // Cursor captured after ingest is >= every created_at, so the delta is
        // empty (strict `>` filtering). The cursor still advances forward.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria").await;
        ingest_extraction(&db, &llm, bd, "maria", extraction(&["Maria"], &[]))
            .await
            .unwrap();

        let since = now_seconds();
        let delta = graph_delta(&db, since).await.unwrap();
        assert!(delta.added_concepts.is_empty(), "no additions after cursor");
        assert!(delta.added_edges.is_empty());
        assert!(delta.deleted_concept_ids.is_empty());
        assert!(delta.deleted_edge_ids.is_empty());
        assert!(delta.retagged_edges.is_empty());
        assert!(
            delta.cursor >= since,
            "cursor is fresh and at least the requested since"
        );
    }

    #[tokio::test]
    async fn delta_excludes_newly_created_edge_from_retags() {
        // An edge created AND retagged after the cursor arrives once — as an
        // addition carrying its current type — not duplicated as a retag.
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria helps q3 launch").await;
        ingest_extraction(
            &db,
            &llm,
            bd,
            "maria helps q3 launch",
            extraction(&["Maria", "Q3 launch"], &[("Maria", "helps", "Q3 launch")]),
        )
        .await
        .unwrap();
        let maria = db_concept_id_for_label(&db, "Maria").await;
        let q3 = db_concept_id_for_label(&db, "Q3 launch").await;
        let edge = find_edge(&db, maria, "helps", q3)
            .await
            .unwrap()
            .expect("edge created");

        // Both creation and retag are after since=0.
        append_retag(&db, edge.id, "supports", now_seconds()).await;

        let delta = graph_delta(&db, 0).await.unwrap();
        let added = delta
            .added_edges
            .iter()
            .find(|e| e.id == edge.id)
            .expect("edge in additions (created after cursor)");
        assert_eq!(
            added.current_type, "supports",
            "addition carries the retagged current type"
        );
        assert!(
            delta.retagged_edges.iter().all(|r| r.id != edge.id),
            "newly-created edge not double-reported as a retag"
        );
    }

    #[tokio::test]
    async fn delta_cursor_advances_so_repeat_query_returns_nothing() {
        // Pull-on-focus loop: a second delta using the first response's cursor
        // returns nothing (no change between the two pulls).
        let db = test_db();
        let llm = fake_llm();
        let bd = seed_braindump(&db, "maria").await;
        ingest_extraction(&db, &llm, bd, "maria", extraction(&["Maria"], &[]))
            .await
            .unwrap();

        let first = graph_delta(&db, 0).await.unwrap();
        assert_eq!(
            first.added_concepts.len(),
            1,
            "first pull returns the addition"
        );
        let second = graph_delta(&db, first.cursor).await.unwrap();
        assert!(
            second.added_concepts.is_empty(),
            "second pull with advanced cursor returns no additions"
        );
        assert!(second.retagged_edges.is_empty());
    }
}
