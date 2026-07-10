import { describe, it, expect, beforeEach, vi } from 'vitest';
import fakeIndexedDB from 'fake-indexeddb';
import {
	createIdb,
	type IdbStore,
	type TopologySnapshot
} from '../../src/lib/state/idb';
import { loadSpatialViewGraph } from '../../src/lib/graph/load';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

const RAW: GlobalTopologySnapshot = {
	concepts: [{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
};

const CACHED: TopologySnapshot = {
	fetchedAt: '2026-06-01T00:00:00Z',
	concepts: [{ id: 'c9', label: 'stale', created_at: '2026-05-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c9', partition_id: 2 }]
};

function apiReturning(raw: GlobalTopologySnapshot) {
	return { getGraph: vi.fn(async () => raw) };
}

function apiThrowing(error: Error) {
	return {
		getGraph: vi.fn(async () => {
			throw error;
		})
	};
}

beforeEach(async () => {
	await new Promise<void>((resolve, reject) => {
		const req = fakeIndexedDB.deleteDatabase('second-brain');
		req.onsuccess = () => resolve();
		req.onerror = () => reject(req.error);
		req.onblocked = () => resolve();
	});
});

describe('loadSpatialViewGraph - network-first with IDB Frozen Graph fallback (ADR-0005)', () => {
	let idb: IdbStore;

	beforeEach(() => {
		idb = createIdb(fakeIndexedDB);
	});

	it('fetches from the backend, stamps fetchedAt, caches in IDB, reports source=network', async () => {
		const api = apiReturning(RAW);
		const { snapshot, source } = await loadSpatialViewGraph(
			api,
			idb,
			() => '2026-07-04T12:00:00Z'
		);
		expect(source).toBe('network');
		expect(snapshot.fetchedAt).toBe('2026-07-04T12:00:00Z');
		expect(snapshot.concepts[0]?.id).toBe('c1');
		const cached = await idb.loadTopologySnapshot();
		expect(cached?.fetchedAt).toBe('2026-07-04T12:00:00Z');
	});

	it('falls back to the cached snapshot (Frozen Graph) when the backend is unreachable', async () => {
		await idb.saveTopologySnapshot(CACHED);
		const api = apiThrowing(new Error('backend unreachable'));
		const { snapshot, source } = await loadSpatialViewGraph(api, idb);
		expect(source).toBe('cache');
		expect(snapshot.concepts[0]?.id).toBe('c9');
	});

	it('throws when the backend is unreachable AND no snapshot is cached', async () => {
		const api = apiThrowing(new Error('down'));
		await expect(loadSpatialViewGraph(api, idb)).rejects.toThrow(
			/unavailable|cached/i
		);
	});

	it('does not overwrite the cache with a failed fetch (no stale-write on error)', async () => {
		await idb.saveTopologySnapshot(CACHED);
		const api = apiThrowing(new Error('down'));
		await loadSpatialViewGraph(api, idb);
		const cached = await idb.loadTopologySnapshot();
		expect(cached?.fetchedAt).toBe(CACHED.fetchedAt);
	});
});
