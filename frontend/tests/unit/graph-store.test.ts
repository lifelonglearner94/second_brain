import { describe, it, expect, beforeEach, vi } from 'vitest';
import fakeIndexedDB from 'fake-indexeddb';
import { createIdb, type IdbStore } from '../../src/lib/state/idb';
import { GraphStore } from '../../src/lib/state/graph.svelte';
import { EDGE_COLOR } from '../../src/lib/graph/build';
import { NO_PARTITION } from '../../src/lib/graph/colors';
import type {
	GlobalTopologySnapshot,
	GraphDelta,
	ChatInferenceProposal,
	ConceptMergeSuggestion,
	OntologyTypeProposal
} from '../../src/lib/api/client';
import type { IngestResponse } from '../../src/lib/capture/ingest';

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

const FIXED_NOW = '2026-07-04T12:00:00Z';
const FIXED_CURSOR = Math.floor(new Date(FIXED_NOW).getTime() / 1000);

function graphApiReturning(raw: GlobalTopologySnapshot) {
	return { getGraph: vi.fn(async () => raw) };
}

function graphApiThrowing(error: Error) {
	return {
		getGraph: vi.fn(async () => {
			throw error;
		})
	};
}

function deltaApiReturning(delta: GraphDelta) {
	return { getGraphDelta: vi.fn(async () => delta) };
}

function deltaApiThrowing(error: Error) {
	return {
		getGraphDelta: vi.fn(async () => {
			throw error;
		})
	};
}

function ingestResponse(overrides: Partial<IngestResponse> = {}): IngestResponse {
	return {
		braindump: { id: '7', created_at: '1790' },
		concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }],
		edges: [
			{
				id: 'e2',
				source_concept_id: 'c3',
				target_concept_id: 'c1',
				original_type: 'disrupts',
				current_type: 'disrupts',
				created_at: '2026-07-04T00:00:00Z'
			}
		],
		cursor: FIXED_CURSOR + 500,
		...overrides
	};
}

function proposal(id: number, source: number, target: number): ChatInferenceProposal {
	return {
		id,
		mode: 'structural_inference',
		source_concept_id: source,
		target_concept_id: target,
		proposed_type: 'endangers',
		evidence_path: [],
		rationale: null,
		status: 'endorsed',
		created_at: 1,
		resolved_at: 1,
		snapshot: null
	};
}

const MERGE_SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: '1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: '2', label: 'melatonin', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: '1',
			target_concept_id: '2',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-02T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: '1', partition_id: 0 },
		{ concept_id: '2', partition_id: 0 }
	]
};

const CONCEPT_SUGGESTION: ConceptMergeSuggestion = {
	id: 11,
	kind: 'concept',
	braindump_id: 5,
	new_concept_label: 'melatonin supplement',
	new_concept_id: 2,
	existing_concept_id: 1,
	similarity: 0.92,
	status: 'pending',
	created_at: 1_700_000_000
};

const TYPE_PROPOSAL: OntologyTypeProposal = {
	id: 33,
	slug: 'endangers',
	label: 'Endangers',
	description: 'Causes harm to.',
	merge_of: 'affects',
	status: 'approved',
	near_match_slug: 'affects',
	near_match_similarity: 0.88
};

describe('GraphStore — the canonical Global Topology Snapshot holder (ADR-0002 store-held)', () => {
	describe('initial state', () => {
		it('starts idle with no snapshot, cursor 0, and an empty view-graph', () => {
			const store = new GraphStore();
			expect(store.status).toBe('idle');
			expect(store.snapshot).toBeNull();
			expect(store.cursor).toBe(0);
			expect(store.data.nodes).toEqual([]);
			expect(store.data.links).toEqual([]);
			expect(store.source).toBeNull();
			expect(store.fetchedAt).toBeNull();
		});
	});

	describe('loadFromNetworkOrCache — bootstrap the canonical snapshot (network-first, IDB Frozen Graph fallback)', () => {
		let idb: IdbStore;

		beforeEach(async () => {
			await new Promise<void>((resolve, reject) => {
				const req = fakeIndexedDB.deleteDatabase('second-brain');
				req.onsuccess = () => resolve();
				req.onerror = () => reject(req.error);
				req.onblocked = () => resolve();
			});
			idb = createIdb(fakeIndexedDB);
		});

		it('fetches from the backend, stamps fetchedAt, caches in IDB, builds the view-graph, reports source=network', async () => {
			const store = new GraphStore();
			const api = graphApiReturning(SNAPSHOT);
			const result = await store.loadFromNetworkOrCache(api, idb, () => FIXED_NOW);

			expect(result.source).toBe('network');
			expect(result.fetchedAt).toBe(FIXED_NOW);
			expect(api.getGraph).toHaveBeenCalledOnce();
			expect(store.source).toBe('network');
			expect(store.fetchedAt).toBe(FIXED_NOW);
			expect(store.status).toBe('loaded');
			expect(store.snapshot?.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2']);
			expect(store.cursor).toBe(FIXED_CURSOR);
			expect(store.data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2']);
			expect(store.data.links.map((l) => l.label)).toEqual(['affects']);
			const cached = await idb.loadTopologySnapshot();
			expect(cached?.fetchedAt).toBe(FIXED_NOW);
		});

		it('falls back to the cached snapshot (Frozen Graph) when the backend is unreachable', async () => {
			await idb.saveTopologySnapshot({ ...SNAPSHOT, fetchedAt: '2026-06-01T00:00:00Z' });
			const store = new GraphStore();
			const api = graphApiThrowing(new Error('backend unreachable'));

			const result = await store.loadFromNetworkOrCache(api, idb);

			expect(result.source).toBe('cache');
			expect(store.source).toBe('cache');
			expect(store.snapshot?.concepts).toHaveLength(2);
			expect(store.data.nodes).toHaveLength(2);
		});

		it('throws when the backend is unreachable AND no snapshot is cached (propagates the Frozen Graph miss)', async () => {
			const store = new GraphStore();
			const api = graphApiThrowing(new Error('down'));
			await expect(store.loadFromNetworkOrCache(api, idb)).rejects.toThrow(/unavailable|cached/i);
			expect(store.snapshot).toBeNull();
		});

		it('is idempotent — a second call does NOT re-fetch (no re-fetch of the Global Topology Snapshot)', async () => {
			const store = new GraphStore();
			const api = graphApiReturning(SNAPSHOT);
			await store.loadFromNetworkOrCache(api, idb, () => FIXED_NOW);
			await store.loadFromNetworkOrCache(api, idb, () => FIXED_NOW);
			expect(api.getGraph).toHaveBeenCalledOnce();
		});

		it('retries after a failed load (a prior miss leaves the store empty, so the next call fetches again)', async () => {
			const store = new GraphStore();
			const throwingApi = graphApiThrowing(new Error('down'));
			await expect(store.loadFromNetworkOrCache(throwingApi, idb)).rejects.toThrow();
			const okApi = graphApiReturning(SNAPSHOT);
			await store.loadFromNetworkOrCache(okApi, idb, () => FIXED_NOW);
			expect(okApi.getGraph).toHaveBeenCalledOnce();
			expect(store.snapshot?.concepts).toHaveLength(2);
		});
	});

	describe('syncDelta — pull-on-focus reconciliation of the canonical snapshot', () => {
		it('fetches changes since the cursor, reconciles the snapshot, advances the cursor, and rebuilds the view-graph', async () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.cursor = FIXED_CURSOR;
			const delta: GraphDelta = {
				cursor: FIXED_CURSOR + 500,
				added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' }],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			};
			const api = deltaApiReturning(delta);

			const outcome = await store.syncDelta(api);

			expect(api.getGraphDelta).toHaveBeenCalledWith(FIXED_CURSOR);
			expect(outcome.applied).toBe(true);
			expect(outcome.delta).toEqual(delta);
			expect(store.cursor).toBe(FIXED_CURSOR + 500);
			expect(store.snapshot?.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2', 'c3']);
			expect(store.data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2', 'c3']);
		});

		it('is a no-op (applied=false) when the store has no snapshot loaded', async () => {
			const store = new GraphStore();
			const api = deltaApiReturning({
				cursor: FIXED_CURSOR + 500,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
			const outcome = await store.syncDelta(api);
			expect(outcome.applied).toBe(false);
			expect(api.getGraphDelta).not.toHaveBeenCalled();
			expect(store.cursor).toBe(0);
		});

		it('leaves the snapshot and cursor untouched when the backend is unreachable (brief staleness between focus events)', async () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.cursor = FIXED_CURSOR;
			const api = deltaApiThrowing(new Error('backend unreachable'));

			const outcome = await store.syncDelta(api);

			expect(outcome.applied).toBe(false);
			expect(store.cursor).toBe(FIXED_CURSOR);
			expect(store.snapshot?.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2']);
		});

		it('does not rebuild the view-graph when the delta carries no changes (empty sync)', async () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.cursor = FIXED_CURSOR;
			const dataBefore = store.data;
			const api = deltaApiReturning({
				cursor: FIXED_CURSOR + 500,
				added_concepts: [],
				added_edges: [],
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
			await store.syncDelta(api);
			expect(store.cursor).toBe(FIXED_CURSOR + 500);
			expect(store.data).toBe(dataBefore);
		});
	});

	describe('mergeIngest — optimistic-merge of a braindump ingestion response (applyDelta with empty deletes/retags)', () => {
		it('appends newly-extracted concepts and edges, advances the cursor, and rebuilds the view-graph', () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.cursor = FIXED_CURSOR;

			store.mergeIngest(ingestResponse());

			expect(store.cursor).toBe(FIXED_CURSOR + 500);
			expect(store.snapshot?.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2', 'c3']);
			expect(store.snapshot?.edges.map((e) => e.id).sort()).toEqual(['e1', 'e2']);
			expect(store.data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2', 'c3']);
		});

		it('leaves the Louvain partitions untouched — new concepts get NO_PARTITION until the next sync', () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.mergeIngest(ingestResponse());
			expect(store.snapshot?.partitions).toEqual(SNAPSHOT.partitions);
			expect(store.data.nodes.find((n) => n.id === 'c3')?.group).toBe(NO_PARTITION);
		});

		it('is a no-op when the store has no snapshot loaded', () => {
			const store = new GraphStore();
			store.mergeIngest(ingestResponse());
			expect(store.snapshot).toBeNull();
			expect(store.cursor).toBe(0);
		});

		it('does not mutate the prior snapshot reference (pure merge — the next Delta Sync overwrites the view)', () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			const before = JSON.parse(JSON.stringify(store.snapshot)) as GlobalTopologySnapshot;
			store.mergeIngest(ingestResponse());
			expect(before.concepts).toHaveLength(2);
			expect(before.edges).toHaveLength(1);
		});
	});

	describe('mergeEndorsedEdge — action-driven optimistic merge of an endorsed chat-inference edge', () => {
		it('optimistically merges an endorsed edge into the view-graph', () => {
			const store = new GraphStore();
			store.loadSnapshot(MERGE_SNAPSHOT);
			store.mergeEndorsedEdge(proposal(101, 1, 2));
			expect(store.data.links).toHaveLength(2);
			const merged = store.data.links.find((l) => l.source === '1' && l.target === '2' && l.label === 'endangers');
			expect(merged).toBeTruthy();
			expect(merged?.asserted_by).toEqual([101]);
			expect(merged?.color).toBe(EDGE_COLOR);
		});

		it('is a no-op when the endpoints are absent (defensive against a partial view)', () => {
			const store = new GraphStore();
			store.loadSnapshot(MERGE_SNAPSHOT);
			const linksBefore = store.data.links.length;
			store.mergeEndorsedEdge(proposal(101, 1, 99));
			expect(store.data.links).toHaveLength(linksBefore);
		});

		it('accumulates successive approvals into the view-graph', () => {
			const store = new GraphStore();
			store.loadSnapshot({
				...MERGE_SNAPSHOT,
				concepts: [...MERGE_SNAPSHOT.concepts, { id: '3', label: 'caffeine', created_at: 't' }]
			});
			store.mergeEndorsedEdge(proposal(101, 1, 2));
			store.mergeEndorsedEdge(proposal(102, 1, 3));
			expect(store.data.links.filter((l) => l.asserted_by?.includes(101))).toHaveLength(1);
			expect(store.data.links.filter((l) => l.asserted_by?.includes(102))).toHaveLength(1);
		});
	});

	describe('applyConceptMerge — fold a duplicate concept into the survivor (housekeeping)', () => {
		it('folds new_concept_id into existing_concept_id and rebuilds the view-graph', () => {
			const store = new GraphStore();
			store.loadSnapshot(MERGE_SNAPSHOT);
			store.applyConceptMerge(CONCEPT_SUGGESTION);
			expect(store.snapshot?.concepts.map((c) => c.id).sort()).toEqual(['1']);
			expect(store.data.nodes.map((n) => n.id).sort()).toEqual(['1']);
		});

		it('retargets edges that pointed at the folded concept to the survivor', () => {
			const store = new GraphStore();
			store.loadSnapshot({
				concepts: [
					{ id: '1', label: 'sleep', created_at: 't' },
					{ id: '2', label: 'melatonin', created_at: 't' },
					{ id: '3', label: 'caffeine', created_at: 't' }
				],
				edges: [
					{
						id: 'e1',
						source_concept_id: '2',
						target_concept_id: '3',
						original_type: 'disrupts',
						current_type: 'disrupts',
						created_at: 't'
					}
				],
				partitions: []
			});
			store.applyConceptMerge(CONCEPT_SUGGESTION);
			expect(store.snapshot?.edges.find((e) => e.id === 'e1')?.source_concept_id).toBe('1');
			expect(store.snapshot?.edges.find((e) => e.id === 'e1')?.target_concept_id).toBe('3');
		});

		it('preserves the fetchedAt stamp across the merge (the Frozen Graph label does not reset)', () => {
			const store = new GraphStore();
			(store as unknown as { fetchedAt: string | null }).fetchedAt = FIXED_NOW;
			store.loadSnapshot(MERGE_SNAPSHOT);
			store.applyConceptMerge(CONCEPT_SUGGESTION);
			expect(store.fetchedAt).toBe(FIXED_NOW);
		});

		it('is a no-op when the store has no snapshot loaded', () => {
			const store = new GraphStore();
			store.applyConceptMerge(CONCEPT_SUGGESTION);
			expect(store.snapshot).toBeNull();
		});
	});

	describe('applyTypeMerge — retag edges of a merged ontology type to the new slug (housekeeping)', () => {
		it('retags every edge of the merge_of type to the new slug and rebuilds the view-graph', () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			store.applyTypeMerge(TYPE_PROPOSAL);
			expect(store.snapshot?.edges.find((e) => e.id === 'e1')?.current_type).toBe('endangers');
			expect(store.data.links.find((l) => l.source === 'c1' && l.target === 'c2')?.label).toBe('endangers');
		});

		it('leaves edges of other types untouched', () => {
			const store = new GraphStore();
			store.loadSnapshot({
				...SNAPSHOT,
				edges: [
					...SNAPSHOT.edges,
					{
						id: 'e2',
						source_concept_id: 'c2',
						target_concept_id: 'c1',
						original_type: 'disrupts',
						current_type: 'disrupts',
						created_at: 't'
					}
				]
			});
			store.applyTypeMerge(TYPE_PROPOSAL);
			expect(store.snapshot?.edges.find((e) => e.id === 'e2')?.current_type).toBe('disrupts');
		});

		it('preserves the fetchedAt stamp across the merge', () => {
			const store = new GraphStore();
			store.loadSnapshot(SNAPSHOT);
			(store as unknown as { fetchedAt: string | null }).fetchedAt = FIXED_NOW;
			store.applyTypeMerge(TYPE_PROPOSAL);
			expect(store.fetchedAt).toBe(FIXED_NOW);
		});

		it('is a no-op when the store has no snapshot loaded', () => {
			const store = new GraphStore();
			store.applyTypeMerge(TYPE_PROPOSAL);
			expect(store.snapshot).toBeNull();
		});
	});
});
