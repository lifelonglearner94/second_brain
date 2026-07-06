import { describe, it, expect, vi } from 'vitest';
import {
	syncDelta,
	onWindowFocus,
	type DeltaSyncState,
	type DeltaSyncApi,
	type DeltaSyncOutcome
} from '../../src/lib/graph/delta-sync';
import type {
	GlobalTopologySnapshot,
	GraphDelta
} from '../../src/lib/api/client';

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

describe('syncDelta — pull-on-focus Delta Sync orchestrator', () => {
	describe('cursor advancement (last_sync_timestamp)', () => {
		it('fetches changes since the cursor and advances last_sync_timestamp to the fresh cursor on success', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
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
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
				added_edges: [],
				deleted_concept_ids: ['c2'],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const api: DeltaSyncApi = { getGraphDelta: vi.fn(async () => delta) };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(outcome.applied).toBe(true);
			expect(outcome.state.snapshot.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c3'
			]);
			if (outcome.applied) {
				expect(outcome.delta).toEqual(delta);
			}
		});
	});

	describe('graceful failure (ADR-0002: brief staleness between focus events)', () => {
		it('leaves the snapshot and cursor untouched when the backend is unreachable, and reports applied=false', async () => {
			const api: DeltaSyncApi = {
				getGraphDelta: vi.fn(async () => {
					throw new Error('backend unreachable');
				})
			};
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(outcome.applied).toBe(false);
			expect(outcome.state.cursor).toBe(1700000000);
			expect(outcome.state.snapshot).toBe(SNAPSHOT);
		});
	});

	describe('empty delta (nothing changed since the cursor)', () => {
		it('still advances the cursor and reports applied=true on a successful fetch', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const api: DeltaSyncApi = { getGraphDelta: vi.fn(async () => delta) };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(outcome.applied).toBe(true);
			expect(outcome.state.cursor).toBe(1700000500);
			expect(outcome.state.snapshot.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c2'
			]);
		});
	});

	describe('onWindowFocus — pull-on-focus event wiring (no WebSocket/SSE/polling)', () => {
		it('calls the callback when the target regains focus', () => {
			const target = new EventTarget();
			const calls: number[] = [];
			const stop = onWindowFocus(target, () => calls.push(1));

			target.dispatchEvent(new Event('focus'));

			expect(calls).toHaveLength(1);
			stop();
		});

		it('stops listening after the returned unsubscribe is called', () => {
			const target = new EventTarget();
			const calls: number[] = [];
			const stop = onWindowFocus(target, () => calls.push(1));

			stop();
			target.dispatchEvent(new Event('focus'));

			expect(calls).toHaveLength(0);
		});
	});

	describe('focus-triggered Delta Sync (acceptance: window focus fetches changes since last_sync_timestamp)', () => {
		it('a window focus event triggers a delta fetch against the current cursor and reconciles the view', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
				added_concepts: [
					{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }
				],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const getGraphDelta = vi.fn(async () => delta);
			const api: DeltaSyncApi = { getGraphDelta };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };
			const target = new EventTarget();

			const holder: { outcome: DeltaSyncOutcome | null } = { outcome: null };
			const stop = onWindowFocus(target, () => {
				void syncDelta(state, api).then((o) => (holder.outcome = o));
			});

			target.dispatchEvent(new Event('focus'));
			await Promise.resolve();
			await Promise.resolve();
			stop();

			const outcome = holder.outcome;
			expect(getGraphDelta).toHaveBeenCalledWith(1700000000);
			expect(outcome?.applied).toBe(true);
			expect(outcome?.state.cursor).toBe(1700000500);
			expect(outcome?.state.snapshot.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c2',
				'c3'
			]);
		});
	});

	describe('Delta Sync overlaid after a braindump ingestion response (acceptance)', () => {
		it('overlays the delta onto the post-ingestion view so the new concepts/edges appear', async () => {
			const delta: GraphDelta = {
				cursor: 1700000500,
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
			const api: DeltaSyncApi = { getGraphDelta: vi.fn(async () => delta) };
			const state: DeltaSyncState = { snapshot: SNAPSHOT, cursor: 1700000000 };

			const outcome = await syncDelta(state, api);

			expect(api.getGraphDelta).toHaveBeenCalledWith(1700000000);
			expect(outcome.state.snapshot.concepts.map((c) => c.id).sort()).toEqual([
				'c1',
				'c2',
				'c3'
			]);
			expect(outcome.state.snapshot.edges.map((e) => e.id).sort()).toEqual([
				'e1',
				'e2'
			]);
			expect(outcome.state.cursor).toBe(1700000500);
		});
	});
});
