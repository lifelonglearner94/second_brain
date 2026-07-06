import { describe, it, expect, beforeEach } from 'vitest';
import fakeIndexedDB from 'fake-indexeddb';
import {
	createIdb,
	type PendingCapture,
	type TopologySnapshot
} from '../../src/lib/state/idb';

const SNAPSHOT: TopologySnapshot = {
	fetchedAt: '2026-07-04T00:00:00Z',
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
		{ concept_id: 'c2', partition_id: 1 }
	]
};

beforeEach(async () => {
	await new Promise<void>((resolve, reject) => {
		const req = fakeIndexedDB.deleteDatabase('second-brain');
		req.onsuccess = () => resolve();
		req.onerror = () => reject(req.error);
		req.onblocked = () => resolve();
	});
});

describe('idb — Global Topology Snapshot cache home', () => {
	it('round-trips a single cached snapshot (read cache, ADR-0005)', async () => {
		const idb = createIdb(fakeIndexedDB);
		await idb.saveTopologySnapshot(SNAPSHOT);
		const loaded = await idb.loadTopologySnapshot();
		expect(loaded).toEqual(SNAPSHOT);
	});

	it('overwrites the previous snapshot (single-slot read cache)', async () => {
		const idb = createIdb(fakeIndexedDB);
		await idb.saveTopologySnapshot(SNAPSHOT);
		const newer: TopologySnapshot = {
			...SNAPSHOT,
			fetchedAt: '2026-07-05T00:00:00Z'
		};
		await idb.saveTopologySnapshot(newer);
		const loaded = await idb.loadTopologySnapshot();
		expect(loaded?.fetchedAt).toBe('2026-07-05T00:00:00Z');
	});

	it('returns undefined when nothing has been cached', async () => {
		const idb = createIdb(fakeIndexedDB);
		expect(await idb.loadTopologySnapshot()).toBeUndefined();
	});

	it('can clear the cached snapshot (for the Frozen Graph cache invalidation)', async () => {
		const idb = createIdb(fakeIndexedDB);
		await idb.saveTopologySnapshot(SNAPSHOT);
		await idb.clearTopologySnapshot();
		expect(await idb.loadTopologySnapshot()).toBeUndefined();
	});
});

describe('idb — Pending Captures home (write-intent, the named exception)', () => {
	it('appends a pending capture and lists captures in insertion order', async () => {
		const idb = createIdb(fakeIndexedDB);
		const first: PendingCapture = {
			id: 'a',
			text: 'first',
			createdAt: '2026-07-04T01:00:00Z'
		};
		const second: PendingCapture = {
			id: 'b',
			text: 'second',
			createdAt: '2026-07-04T02:00:00Z'
		};
		await idb.enqueuePendingCapture(first);
		await idb.enqueuePendingCapture(second);
		expect(await idb.listPendingCaptures()).toEqual([first, second]);
	});

	it('removes a pending capture by id (after review-and-confirm)', async () => {
		const idb = createIdb(fakeIndexedDB);
		await idb.enqueuePendingCapture({
			id: 'a',
			text: 'first',
			createdAt: 't1'
		});
		await idb.enqueuePendingCapture({
			id: 'b',
			text: 'second',
			createdAt: 't2'
		});
		await idb.removePendingCapture('a');
		const remaining = await idb.listPendingCaptures();
		expect(remaining).toHaveLength(1);
		expect(remaining[0].id).toBe('b');
	});

	it('lists an empty queue as []', async () => {
		const idb = createIdb(fakeIndexedDB);
		expect(await idb.listPendingCaptures()).toEqual([]);
	});
});
