import { describe, it, expect, vi } from 'vitest';
import { ActiveCaptureStore } from '../../src/lib/capture/active-capture.svelte';
import { submitActiveCapture } from '../../src/lib/capture/pending';
import { PendingCapturesStore } from '../../src/lib/state/pending-captures.svelte';
import type { IdbStore, PendingCapture } from '../../src/lib/state/idb';
import type { IngestApi, IngestResponse } from '../../src/lib/capture/ingest';

function fakeIdb(): IdbStore & {
	enqueued: PendingCapture[];
} {
	const enqueued: PendingCapture[] = [];
	return {
		enqueued,
		saveTopologySnapshot: vi.fn(),
		loadTopologySnapshot: vi.fn(),
		clearTopologySnapshot: vi.fn(),
		enqueuePendingCapture: vi.fn(async (c: PendingCapture) => {
			enqueued.push(c);
		}),
		listPendingCaptures: vi.fn(async () => [...enqueued]),
		removePendingCapture: vi.fn()
	} as unknown as IdbStore & { enqueued: PendingCapture[] };
}

function fakeIngest(res: IngestResponse): IngestApi & { calls: string[] } {
	const calls: string[] = [];
	return {
		calls,
		async ingest(verbatim: string) {
			calls.push(verbatim);
			return res;
		}
	};
}

const INGESTED: IngestResponse = {
	braindump: { id: 'b1', created_at: '200' },
	concepts: [{ id: 'c2', label: 'caffeine', created_at: '200' }],
	edges: [],
	cursor: 200
};

describe('submitActiveCapture — the offline-enqueue vs #19-ingest branch (ADR-0005/0007)', () => {
	it('enqueues a Pending Capture instead of ingesting when the browser is offline', async () => {
		const idb = fakeIdb();
		const pending = new PendingCapturesStore(idb);
		const active = new ActiveCaptureStore();
		active.text = 'offline capture';
		active.sttSourceLabel = 'deepgram';
		const ingest = fakeIngest(INGESTED);

		const outcome = await submitActiveCapture(active, false, pending, ingest);

		expect(outcome.kind).toBe('queued');
		expect(ingest.calls).toHaveLength(0);
		expect(idb.enqueuePendingCapture).toHaveBeenCalledOnce();
		const captured = (idb.enqueuePendingCapture as ReturnType<typeof vi.fn>).mock.calls[0][0];
		expect(captured.text).toBe('offline capture');
		expect(captured.id).toBeTruthy();
		expect(captured.createdAt).toBeTruthy();
		expect(pending.items).toHaveLength(1);
		expect(pending.items[0]?.text).toBe('offline capture');
		expect(active.text).toBe('');
		expect(active.status).toBe('queued');
	});

	it('enqueues when only the offline STT fallback filled the buffer (web-speech), even while online', async () => {
		const idb = fakeIdb();
		const pending = new PendingCapturesStore(idb);
		const active = new ActiveCaptureStore();
		active.text = 'web speech capture';
		active.sttSourceLabel = 'web-speech';
		const ingest = fakeIngest(INGESTED);

		const outcome = await submitActiveCapture(active, true, pending, ingest);

		expect(outcome.kind).toBe('queued');
		expect(ingest.calls).toHaveLength(0);
		expect(idb.enqueuePendingCapture).toHaveBeenCalledOnce();
		expect(pending.items[0]?.text).toBe('web speech capture');
		expect(active.status).toBe('queued');
	});

	it('submits immediately through the #19 ingest path when online with Deepgram', async () => {
		const idb = fakeIdb();
		const pending = new PendingCapturesStore(idb);
		const active = new ActiveCaptureStore();
		active.text = 'online capture';
		active.sttSourceLabel = 'deepgram';
		const ingest = fakeIngest(INGESTED);

		const outcome = await submitActiveCapture(active, true, pending, ingest);

		expect(outcome.kind).toBe('submitted');
		if (outcome.kind === 'submitted') {
			expect(outcome.res.braindump.id).toBe('b1');
		}
		expect(ingest.calls).toEqual(['online capture']);
		expect(idb.enqueuePendingCapture).not.toHaveBeenCalled();
		expect(pending.items).toHaveLength(0);
		expect(active.text).toBe('');
		expect(active.status).toBe('submitted');
	});

	it('rejects an empty buffer the same way the #19 path does (no enqueue, no ingest)', async () => {
		const idb = fakeIdb();
		const pending = new PendingCapturesStore(idb);
		const active = new ActiveCaptureStore();
		active.text = '   ';
		const ingest = fakeIngest(INGESTED);

		await expect(submitActiveCapture(active, false, pending, ingest)).rejects.toThrow(
			/empty/
		);
		expect(ingest.calls).toHaveLength(0);
		expect(idb.enqueuePendingCapture).not.toHaveBeenCalled();
	});
});
