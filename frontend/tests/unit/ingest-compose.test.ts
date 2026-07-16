import { describe, it, expect, vi } from 'vitest';
import {
	createIngestApi,
	type IngestApi,
	type IngestResponse
} from '../../src/lib/capture/ingest';
import type {
	BraindumpDto,
	GraphDelta,
	IngestStatus
} from '../../src/lib/api/client';

const BRAINDUMP: BraindumpDto = {
	id: '7',
	verbatim: 'caffeine disrupts sleep',
	cleaned: 'Caffeine disrupts sleep.',
	created_at: '1790'
};

const DELTA: GraphDelta = {
	cursor: 1_800,
	added_concepts: [{ id: 'c3', label: 'caffeine', created_at: '1790' }],
	added_edges: [
		{
			id: 'e2',
			source_concept_id: 'c3',
			target_concept_id: 'c1',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '1790'
		}
	],
	deleted_concept_ids: [],
	deleted_edge_ids: [],
	retagged_edges: []
};

const COMPLETE: IngestStatus = {
	status: 'complete',
	attempts: 1,
	last_attempt_at: 1_795
};

const PENDING: IngestStatus = {
	status: 'pending',
	attempts: 0,
	last_attempt_at: null
};

const FAILED: IngestStatus = {
	status: 'failed',
	attempts: 1,
	last_attempt_at: 1_795
};

function clientStub(
	submitBraindump: (v: string) => Promise<BraindumpDto>,
	getGraphDelta: (since?: number) => Promise<GraphDelta>,
	getIngestStatus: (id: number) => Promise<IngestStatus>
) {
	return { submitBraindump, getGraphDelta, getIngestStatus };
}

describe('createIngestApi - POST /braindumps then poll ingest-status → GET /graph/delta (issue #97)', () => {
	it('submits the verbatim, polls ingest-status, then fetches the delta once complete and packages concepts/edges + fresh cursor', async () => {
		const submitBraindump = vi.fn(async () => BRAINDUMP);
		const getGraphDelta = vi.fn(async () => DELTA);
		const getIngestStatus = vi.fn(async () => COMPLETE);
		const ingest: IngestApi = createIngestApi(
			clientStub(submitBraindump, getGraphDelta, getIngestStatus),
			() => 1_780,
			[0]
		);

		const res: IngestResponse = await ingest.ingest('caffeine disrupts sleep');

		expect(submitBraindump).toHaveBeenCalledWith('caffeine disrupts sleep');
		expect(getIngestStatus).toHaveBeenCalledWith(7);
		expect(getGraphDelta).toHaveBeenCalledOnce();
		expect(getGraphDelta).toHaveBeenCalledWith(1_780);
		expect(res.braindump.id).toBe('7');
		expect(res.braindump.created_at).toBe('1790');
		expect(res.concepts[0]?.label).toBe('caffeine');
		expect(res.edges[0]?.current_type).toBe('disrupts');
		expect(res.cursor).toBe(1_800);
	});

	it('returns the freshly-extracted concepts/edges as the optimistic-merge source of truth', async () => {
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				vi.fn(async () => DELTA),
				vi.fn(async () => COMPLETE)
			),
			() => 0,
			[0]
		);
		const res = await ingest.ingest('caffeine disrupts sleep');
		expect(res.concepts).toBe(DELTA.added_concepts);
		expect(res.edges).toBe(DELTA.added_edges);
	});

	it('does not fetch the delta when the verbatim submit itself fails (no point merging nothing)', async () => {
		const submitBraindump = vi.fn(async () => {
			throw new Error('POST /braindumps failed: 400');
		});
		const getGraphDelta = vi.fn(async () => DELTA);
		const getIngestStatus = vi.fn(async () => COMPLETE);
		const ingest = createIngestApi(
			clientStub(submitBraindump, getGraphDelta, getIngestStatus),
			() => 1_780,
			[0]
		);
		await expect(ingest.ingest('x')).rejects.toThrow(/400/);
		expect(getGraphDelta).not.toHaveBeenCalled();
		expect(getIngestStatus).not.toHaveBeenCalled();
	});

	it('surfaces a delta-fetch failure so the caller can fall back to the next focus event', async () => {
		const getGraphDelta = vi.fn(async () => {
			throw new Error('GET /graph/delta failed: 503');
		});
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta,
				vi.fn(async () => COMPLETE)
			),
			() => 1_780,
			[0]
		);
		await expect(ingest.ingest('x')).rejects.toThrow(/503/);
	});

	it('reads the cursor lazily at ingest time so a post-submit cursor bump is seen on the next call', async () => {
		let cursor = 1_780;
		const getGraphDelta = vi.fn(async () => ({ ...DELTA, cursor: 1_900 }));
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta,
				vi.fn(async () => COMPLETE)
			),
			() => cursor,
			[0]
		);
		await ingest.ingest('first');
		cursor = 1_900;
		await ingest.ingest('second');
		expect(getGraphDelta).toHaveBeenNthCalledWith(2, 1_900);
	});

	it('polls ingest-status until complete, then fetches the delta exactly once', async () => {
		const getIngestStatus = vi
			.fn<() => Promise<IngestStatus>>()
			.mockResolvedValueOnce(PENDING)
			.mockResolvedValueOnce(PENDING)
			.mockResolvedValueOnce(COMPLETE);
		const getGraphDelta = vi.fn(async () => DELTA);
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta,
				getIngestStatus
			),
			() => 1_780,
			[0, 0, 0]
		);

		const res = await ingest.ingest('caffeine disrupts sleep');

		expect(getIngestStatus).toHaveBeenCalledTimes(3);
		expect(getGraphDelta).toHaveBeenCalledOnce();
		expect(getGraphDelta).toHaveBeenCalledWith(1_780);
		expect(res.concepts[0]?.label).toBe('caffeine');
		expect(res.edges[0]?.current_type).toBe('disrupts');
		expect(res.cursor).toBe(1_800);
	});

	it('stops polling on failed status without fetching the delta and keeps the cursor unchanged', async () => {
		const getIngestStatus = vi.fn(async () => FAILED);
		const getGraphDelta = vi.fn(async () => DELTA);
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta,
				getIngestStatus
			),
			() => 1_780,
			[0, 0, 0]
		);

		const res = await ingest.ingest('caffeine disrupts sleep');

		expect(getIngestStatus).toHaveBeenCalledOnce();
		expect(getGraphDelta).not.toHaveBeenCalled();
		expect(res.concepts).toEqual([]);
		expect(res.edges).toEqual([]);
		expect(res.cursor).toBe(1_780);
	});

	it('stops polling on timeout without fetching the delta or advancing the cursor', async () => {
		const getIngestStatus = vi.fn(async () => PENDING);
		const getGraphDelta = vi.fn(async () => DELTA);
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta,
				getIngestStatus
			),
			() => 1_780,
			[0, 0]
		);

		const res = await ingest.ingest('caffeine disrupts sleep');

		expect(getIngestStatus).toHaveBeenCalledTimes(2);
		expect(getGraphDelta).not.toHaveBeenCalled();
		expect(res.concepts).toEqual([]);
		expect(res.edges).toEqual([]);
		expect(res.cursor).toBe(1_780);
	});
});
