import { describe, it, expect } from 'vitest';
import { applyDelta } from '../../src/lib/graph/delta';
import { buildGraphData } from '../../src/lib/graph/build';
import { NO_PARTITION } from '../../src/lib/graph/colors';
import type {
	GlobalTopologySnapshot,
	GraphDelta
} from '../../src/lib/api/client';

const BASE: GlobalTopologySnapshot = {
	concepts: [
		{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: 'c2', label: 'melatonin', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: 'c1',
			target_concept_id: 'c2',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-02T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: 'c1', partition_id: 0 },
		{ concept_id: 'c2', partition_id: 0 }
	]
};

describe('applyDelta — reconcile the Spatial View-Graph with a Delta Sync payload', () => {
	describe('apply-additions', () => {
		it('merges added concepts and edges from ingests the user did not trigger', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
				added_edges: [
					{
						id: 'e2',
						source_concept_id: 'c3',
						target_concept_id: 'c1',
						original_type: 'disrupts',
						current_type: 'disrupts',
						created_at: '2026-07-03T00:00:00Z'
					}
				],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c2',
				'c3'
			]);
			expect(reconciled.concepts.find((c) => c.id === 'c3')?.label).toBe(
				'caffeine'
			);
			expect(reconciled.edges.map((e) => e.id).sort()).toEqual(['e1', 'e2']);
			expect(reconciled.edges.find((e) => e.id === 'e2')?.current_type).toBe(
				'disrupts'
			);
		});

		it('preserves the existing concepts and partitions (additions append, never overwrite)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.partitions).toEqual(BASE.partitions);
			expect(reconciled.concepts[0]).toEqual(BASE.concepts[0]);
		});

		it('does not duplicate a concept or edge the snapshot already holds (idempotent re-apply)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c1', label: 'sleep (dup)', created_at: '2026-07-01T00:00:00Z' }
				],
				added_edges: [
					{
						id: 'e1',
						source_concept_id: 'c1',
						target_concept_id: 'c2',
						original_type: 'affects',
						current_type: 'affects',
						created_at: '2026-07-02T00:00:00Z'
					}
				],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.concepts.filter((c) => c.id === 'c1')).toHaveLength(1);
			expect(reconciled.concepts.find((c) => c.id === 'c1')?.label).toBe(
				'sleep'
			);
			expect(reconciled.edges.filter((e) => e.id === 'e1')).toHaveLength(1);
		});
	});

	describe('apply-deletions', () => {
		it('removes tombstoned concepts and edges (vanished via the deletion cascade, ADR-0007/0010)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: ['c2'],
				deleted_edge_ids: ['e1'],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.concepts.map((c) => c.id)).toEqual(['c1']);
			expect(reconciled.edges).toEqual([]);
		});

		it('drops a deleted concept partition assignment so the Louvain view stays consistent', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: ['c1'],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.partitions.map((p) => p.concept_id)).toEqual(['c2']);
		});

		it('also drops edges whose endpoint concept vanished, even if the backend did not list them (defensive against a partial tombstone)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: ['c1'],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.edges).toEqual([]);
		});

		it('ignores deletion ids that are not in the snapshot (no error on a stale cursor)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: ['ghost'],
				deleted_edge_ids: ['phantom'],
				retagged_edges: []
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2']);
			expect(reconciled.edges.map((e) => e.id)).toEqual(['e1']);
		});
	});

	describe('apply-retags', () => {
		it('updates an existing edge current_type to the projected retag (async ontology refactor, ADR-0003)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: [
					{
						id: 'e1',
						source_concept_id: 'c1',
						target_concept_id: 'c2',
						original_type: 'affects',
						current_type: 'endangers'
					}
				]
			};
			const reconciled = applyDelta(BASE, delta);
			const edge = reconciled.edges.find((e) => e.id === 'e1');
			expect(edge?.current_type).toBe('endangers');
			expect(edge?.original_type).toBe('affects');
		});

		it('leaves the original_type immutable so the assertion history is preserved', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: [
					{
						id: 'e1',
						source_concept_id: 'c1',
						target_concept_id: 'c2',
						original_type: 'affects',
						current_type: 'endangers'
					}
				]
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.edges.find((e) => e.id === 'e1')?.original_type).toBe(
				'affects'
			);
		});

		it('ignores a retag for an edge the snapshot does not hold (already deleted / not yet fetched)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: [
					{
						id: 'ghost',
						source_concept_id: 'c1',
						target_concept_id: 'c2',
						original_type: 'affects',
						current_type: 'endangers'
					}
				]
			};
			const reconciled = applyDelta(BASE, delta);
			expect(reconciled.edges).toHaveLength(1);
			expect(reconciled.edges[0]?.current_type).toBe('affects');
		});
	});

	describe('ingest optimistic-merge — applyDelta with empty deletes/retags (ADR-0002)', () => {
		it('appends newly-extracted concepts and edges from a braindump ingestion response', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }
				],
				added_edges: [
					{
						id: 'e2',
						source_concept_id: 'c3',
						target_concept_id: 'c1',
						original_type: 'disrupts',
						current_type: 'disrupts',
						created_at: '2026-07-04T00:00:00Z'
					}
				],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const merged = applyDelta(BASE, delta);
			expect(merged.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c2',
				'c3'
			]);
			expect(merged.edges.map((e) => e.id).sort()).toEqual(['e1', 'e2']);
			expect(merged.edges.find((e) => e.id === 'e2')?.current_type).toBe(
				'disrupts'
			);
		});

		it('leaves the Louvain partitions untouched — new concepts get NO_PARTITION until the next sync (ADR-0008)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }
				],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const merged = applyDelta(BASE, delta);
			expect(merged.partitions).toEqual(BASE.partitions);
			expect(
				merged.partitions.find((p) => p.concept_id === 'c3')
			).toBeUndefined();
		});

		it('does not mutate the input snapshot (pure merge — the next Delta Sync overwrites the view)', () => {
			const before = JSON.parse(JSON.stringify(BASE)) as GlobalTopologySnapshot;
			applyDelta(BASE, {
				cursor: 1700000000,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: 't' }],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
			expect(BASE).toEqual(before);
		});

		it('a freshly-added concept falls back to NO_PARTITION in the rendered graph (no client-side Louvain)', () => {
			const merged = applyDelta(BASE, {
				cursor: 1700000000,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: 't' }],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
			const data = buildGraphData(merged);
			expect(data.nodes.find((n) => n.id === 'c3')?.group).toBe(NO_PARTITION);
		});

		it('an empty ingest is a no-op (the view is unchanged)', () => {
			const merged = applyDelta(BASE, {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
			expect(merged).toEqual(BASE);
		});
	});

	describe('reconciliation reaches the graphology Spatial View-Graph via buildGraphData', () => {
		it('added concepts and edges appear as graphology nodes and typed links', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
				added_edges: [
					{
						id: 'e2',
						source_concept_id: 'c3',
						target_concept_id: 'c1',
						original_type: 'disrupts',
						current_type: 'disrupts',
						created_at: '2026-07-03T00:00:00Z'
					}
				],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const data = buildGraphData(applyDelta(BASE, delta));
			expect(data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2', 'c3']);
			expect(data.nodes.find((n) => n.id === 'c3')?.label).toBe('caffeine');
			const disrupts = data.links.find(
				(l) => l.source === 'c3' && l.target === 'c1'
			);
			expect(disrupts?.label).toBe('disrupts');
		});

		it('deleted concepts and edges are gone from the graphology graph', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: ['c2'],
				deleted_edge_ids: ['e1'],
				retagged_edges: []
			};
			const data = buildGraphData(applyDelta(BASE, delta));
			expect(data.nodes.map((n) => n.id)).toEqual(['c1']);
			expect(data.links).toEqual([]);
		});

		it('a retagged edge renders with the new current_type as its link label', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: [
					{
						id: 'e1',
						source_concept_id: 'c1',
						target_concept_id: 'c2',
						original_type: 'affects',
						current_type: 'endangers'
					}
				]
			};
			const data = buildGraphData(applyDelta(BASE, delta));
			const link = data.links.find(
				(l) => l.source === 'c1' && l.target === 'c2'
			);
			expect(link?.label).toBe('endangers');
		});

		it('an empty delta leaves the rebuilt graphology graph unchanged', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const before = buildGraphData(BASE);
			const after = buildGraphData(applyDelta(BASE, delta));
			expect(after.nodes.map((n) => n.id).sort()).toEqual(
				before.nodes.map((n) => n.id).sort()
			);
			expect(
				after.links.map((l) => `${l.source}-${l.label}-${l.target}`).sort()
			).toEqual(
				before.links.map((l) => `${l.source}-${l.label}-${l.target}`).sort()
			);
		});
	});
});
