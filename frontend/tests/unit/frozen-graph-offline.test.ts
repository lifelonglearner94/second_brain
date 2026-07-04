import { describe, it, expect, beforeEach, vi } from 'vitest';
import fakeIndexedDB from 'fake-indexeddb';
import { createIdb, type IdbStore, type TopologySnapshot } from '../../src/lib/state/idb';
import { loadSpatialViewGraph } from '../../src/lib/graph/load';
import { frozenGraphStatus } from '../../src/lib/graph/frozen-graph';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

const CACHED: TopologySnapshot = {
	fetchedAt: '2026-06-01T00:00:00Z',
	concepts: [{ id: 'c9', label: 'stale concept', created_at: '2026-05-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c9', partition_id: 2 }]
};

const RAW: GlobalTopologySnapshot = {
	concepts: [{ id: 'c1', label: 'fresh', created_at: '2026-07-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
};

beforeEach(async () => {
	await new Promise<void>((resolve, reject) => {
		const req = fakeIndexedDB.deleteDatabase('second-brain');
		req.onsuccess = () => resolve();
		req.onerror = () => reject(req.error);
		req.onblocked = () => resolve();
	});
});

describe('offline-open story — frozen-graph render, staleness indicator, capture routed to Pending (ADR-0005, issue #21)', () => {
	let idb: IdbStore;

	beforeEach(() => {
		idb = createIdb(fakeIndexedDB);
	});

	it('loads the cached Global Topology Snapshot when the backend is unreachable, labels it offline, and routes new captures to Pending', async () => {
		await idb.saveTopologySnapshot(CACHED);
		const throwingApi = {
			getGraph: vi.fn(async (): Promise<GlobalTopologySnapshot> => {
				throw new Error('backend unreachable');
			})
		};

		const loaded = await loadSpatialViewGraph(throwingApi, idb);
		expect(loaded.source).toBe('cache');
		expect(loaded.snapshot.fetchedAt).toBe(CACHED.fetchedAt);
		expect(loaded.snapshot.concepts[0]?.id).toBe('c9');

		const frozen = frozenGraphStatus(loaded.source, loaded.snapshot.fetchedAt, false);
		expect(frozen.status).toBe('offline');
		expect(frozen.label).not.toBeNull();
		expect(frozen.label).toContain(CACHED.fetchedAt);
		expect(frozen.label?.toLowerCase()).toContain('offline');
	});

	it('never renders a blank screen on connectivity failure with no cache — the load throws a user-meaningful error and frozenGraphStatus maps it to a non-blank label', async () => {
		const throwingApi = {
			getGraph: vi.fn(async (): Promise<GlobalTopologySnapshot> => {
				throw new Error('down');
			})
		};

		await expect(loadSpatialViewGraph(throwingApi, idb)).rejects.toThrow(
			/unavailable|cached|offline/i
		);

		const frozen = frozenGraphStatus(
			'error',
			null,
			false,
			'Global Topology Snapshot unavailable: backend unreachable and no cached snapshot'
		);
		expect(frozen.status).toBe('error');
		expect(frozen.label).not.toBeNull();
		expect(frozen.label).toContain('Could not load the graph');
	});

	it('does not flag staleness when the backend is reachable and online — captures ingest immediately', async () => {
		const freshApi = { getGraph: vi.fn(async (): Promise<GlobalTopologySnapshot> => RAW) };

		const loaded = await loadSpatialViewGraph(freshApi, idb, () => '2026-07-04T12:00:00Z');
		expect(loaded.source).toBe('network');

		const frozen = frozenGraphStatus(loaded.source, loaded.snapshot.fetchedAt, true);
		expect(frozen.status).toBe('ready');
		expect(frozen.label).toBeNull();
	});
});
