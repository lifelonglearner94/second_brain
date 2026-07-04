import { describe, it, expect } from 'vitest';
import { applyDelta } from '../../src/lib/graph/delta';
import { buildGraphData } from '../../src/lib/graph/build';
import type { GlobalTopologySnapshot, GraphDelta } from '../../src/lib/api/client';

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
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }],
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
			expect(reconciled.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2', 'c3']);
			expect(reconciled.concepts.find((c) => c.id === 'c3')?.label).toBe('caffeine');
			expect(reconciled.edges.map((e) => e.id).sort()).toEqual(['e1', 'e2']);
			expect(reconciled.edges.find((e) => e.id === 'e2')?.current_type).toBe('disrupts');
		});

		it('preserves the existing concepts and partitions (additions append, never overwrite)', () => {
			const delta: GraphDelta = {
				cursor: 1700000000,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }],
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
				added_concepts: [{ id: 'c1', label: 'sleep (dup)', created_at: '2026-07-01T00:00:00Z' }],
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
			expect(reconciled.concepts.find((c) => c.id === 'c1')?.label).toBe('sleep');
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
			expect(reconciled.edges.find((e) => e.id === 'e1')?.original_type).toBe('affects');
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
});
