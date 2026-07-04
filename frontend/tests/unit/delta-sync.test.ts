import { describe, it, expect, vi } from 'vitest';
import { syncDelta, type DeltaSyncState, type DeltaSyncApi } from '../../src/lib/graph/delta-sync';
import type { GlobalTopologySnapshot, GraphDelta } from '../../src/lib/api/client';

const SNAPSHOT: GlobalTopologySnapshot = {
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

function apiReturning(delta: GraphDelta): DeltaSyncApi {
	return { getGraphDelta: vi.fn(async () => delta) };
}

describe('syncDelta — pull-on-focus Delta Sync orchestrator', () => {
	describe('cursor advancement (last_sync_timestamp)', () => {
		it('fetches changes since the cursor and advances last_sync_timestamp to the fresh cursor on success', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const getGraphDelta = vi.fn(async () => delta);
			const api: DeltaSyncApi = { getGraphDelta };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(getGraphDelta).toHaveBeenCalledWith(1700000000);
			expect(outcome.applied).toBe(true);
			expect(outcome.state.cursor).toBe(1700000500);
		});

		it('reconciles the snapshot with the fetched delta', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }],
				added_edges: [],
				deleted_concept_ids: ['c2'],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const api: DeltaSyncApi = { getGraphDelta: vi.fn(async () => delta) };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(outcome.state.snapshot.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c3']);
			expect(outcome.delta).toEqual(delta);
		});
	});
});
