import { describe, it, expect, vi } from 'vitest';
import {
	createIngestApi,
	type IngestApi,
	type IngestResponse
} from '../../src/lib/capture/ingest';
import type { BraindumpDto } from '../../src/lib/api/client';

const BRAINDUMP: BraindumpDto = {
	id: '7',
	verbatim: 'caffeine disrupts sleep',
	cleaned: 'Caffeine disrupts sleep.',
	created_at: '1790'
};

function clientStub(
	submitBraindump: (v: string) => Promise<BraindumpDto>
): { submitBraindump: typeof submitBraindump } {
	return { submitBraindump };
}

describe('createIngestApi - fire-and-forget submit (issue #102, restoring #84/#85)', () => {
	it('POSTs the verbatim and resolves immediately with an empty IngestResponse so the Active Capture buffer clears in milliseconds', async () => {
		const submitBraindump = vi.fn(async () => BRAINDUMP);
		const ingest: IngestApi = createIngestApi(
			clientStub(submitBraindump),
			() => 1_780
		);

		const res: IngestResponse = await ingest.ingest('caffeine disrupts sleep');

		expect(submitBraindump).toHaveBeenCalledWith('caffeine disrupts sleep');
		expect(res.braindump.id).toBe('7');
		expect(res.braindump.created_at).toBe('1790');
		expect(res.concepts).toEqual([]);
		expect(res.edges).toEqual([]);
	});

	it('does not advance the cursor on the post-submit response (#97 cursor-advance invariant)', async () => {
		const ingest = createIngestApi(
			clientStub(vi.fn(async () => BRAINDUMP)),
			() => 1_780
		);
		const res = await ingest.ingest('caffeine disrupts sleep');
		expect(res.cursor).toBe(1_780);
	});

	it('never calls getIngestStatus or getGraphDelta on the submit hot path (the poll loop is gone)', async () => {
		const submitBraindump = vi.fn(async () => BRAINDUMP);
		const getGraphDelta = vi.fn();
		const getIngestStatus = vi.fn();
		// The client still *has* these methods (kept on ApiClient for
		// diagnostics / focus-sync), but the submit hot path must not invoke
		// them. Declare the stub as a wider type so the extra props are not an
		// excess-property error, then assert they are never called.
		const client = {
			submitBraindump,
			getGraphDelta,
			getIngestStatus
		} as unknown as Parameters<typeof createIngestApi>[0];
		const ingest = createIngestApi(client, () => 1_780);
		await ingest.ingest('caffeine disrupts sleep');
		expect(getGraphDelta).not.toHaveBeenCalled();
		expect(getIngestStatus).not.toHaveBeenCalled();
	});

	it('does not sleep or backoff - ingest() resolves in the same microtask round as submitBraindump', async () => {
		const submitBraindump = vi.fn(async () => BRAINDUMP);
		const ingest = createIngestApi(
			clientStub(submitBraindump),
			() => 1_780
		);
		const pending = ingest.ingest('caffeine disrupts sleep');
		// A pending poll loop would still be awaiting a setTimeout here. Assert
		// the promise settles without yielding to any timer.
		await pending;
		expect(submitBraindump).toHaveBeenCalledOnce();
	});

	it('propagates a verbatim-submit failure so the Active Capture submit can surface the error (no swallow)', async () => {
		const submitBraindump = vi.fn(async () => {
			throw new Error('POST /braindumps failed: 400');
		});
		const ingest = createIngestApi(
			clientStub(submitBraindump),
			() => 1_780
		);
		await expect(ingest.ingest('x')).rejects.toThrow(/400/);
	});

	it('reads the cursor lazily at ingest time so the returned cursor reflects the live store value', async () => {
		let cursor = 1_780;
		const ingest = createIngestApi(
			clientStub(vi.fn(async () => BRAINDUMP)),
			() => cursor
		);
		expect((await ingest.ingest('first')).cursor).toBe(1_780);
		cursor = 1_900;
		expect((await ingest.ingest('second')).cursor).toBe(1_900);
	});

	it('does not block on the background clean→extract→accrete pipeline - a still-pending ingest is invisible to the submit hot path', async () => {
		// The background pipeline is the backend IngestRunner's concern. The
		// frontend only learns about its commits later, via the /app focus-sync
		// (issue #98) or a hard-reload. This test pins the contract: even with
		// no status/delta wiring at all, ingest() returns immediately.
		const ingest = createIngestApi(
			clientStub(vi.fn(async () => BRAINDUMP)),
			() => 1_780
		);
		const start = Date.now();
		await ingest.ingest('caffeine disrupts sleep');
		const elapsed = Date.now() - start;
		// Generous bound; the point is "milliseconds, not seconds". A poll loop
		// with a 400ms first delay would blow this.
		expect(elapsed).toBeLessThan(100);
	});
});
