import { describe, it, expect, beforeEach } from 'vitest';
import fakeIndexedDB from 'fake-indexeddb';
import { createIdb, type PendingCapture } from '../../src/lib/state/idb';
import { PendingCapturesStore } from '../../src/lib/state/pending-captures.svelte';

const FIRST: PendingCapture = {
	id: 'a',
	text: 'offline thought one',
	createdAt: '2026-07-04T01:00:00Z'
};

const SECOND: PendingCapture = {
	id: 'b',
	text: 'offline thought two',
	createdAt: '2026-07-04T02:00:00Z'
};

beforeEach(async () => {
	await new Promise<void>((resolve, reject) => {
		const req = fakeIndexedDB.deleteDatabase('second-brain');
		req.onsuccess = () => resolve();
		req.onerror = () => reject(req.error);
		req.onblocked = () => resolve();
	});
});

describe('PendingCapturesStore — the offline write-intent queue surface (ADR-0005)', () => {
	it('starts empty with count 0', () => {
		const store = new PendingCapturesStore(createIdb(fakeIndexedDB));
		expect(store.items).toEqual([]);
		expect(store.count).toBe(0);
	});

	it('enqueue() records a capture in IndexedDB and surfaces it in items', async () => {
		const idb = createIdb(fakeIndexedDB);
		const store = new PendingCapturesStore(idb);
		await store.enqueue(FIRST);
		expect(store.items).toEqual([FIRST]);
		expect(store.count).toBe(1);
		expect(await idb.listPendingCaptures()).toEqual([FIRST]);
	});

	it('load() surfaces queued captures on reconnect (the Pending Captures review list)', async () => {
		const idb = createIdb(fakeIndexedDB);
		await idb.enqueuePendingCapture(FIRST);
		await idb.enqueuePendingCapture(SECOND);
		const store = new PendingCapturesStore(idb);
		expect(store.items).toEqual([]);
		await store.load();
		expect(store.items).toEqual([FIRST, SECOND]);
		expect(store.count).toBe(2);
	});

	it('remove() drops a confirmed capture from the queue and from IndexedDB', async () => {
		const idb = createIdb(fakeIndexedDB);
		const store = new PendingCapturesStore(idb);
		await store.enqueue(FIRST);
		await store.enqueue(SECOND);
		await store.remove(FIRST.id);
		expect(store.items.map((c) => c.id)).toEqual([SECOND.id]);
		expect(await idb.listPendingCaptures()).toEqual([SECOND]);
	});

	it('remove() leaves an empty list when the last capture is confirmed', async () => {
		const idb = createIdb(fakeIndexedDB);
		const store = new PendingCapturesStore(idb);
		await store.enqueue(FIRST);
		await store.remove(FIRST.id);
		expect(store.items).toEqual([]);
		expect(store.count).toBe(0);
		expect(await idb.listPendingCaptures()).toEqual([]);
	});
});
