import { describe, it, expect, vi } from 'vitest';
import {
	createIngestApi,
	type IngestApi,
	type IngestResponse
} from '../../src/lib/capture/ingest';
import type { BraindumpDto, GraphDelta } from '../../src/lib/api/client';

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

function clientStub(
	submitBraindump: (v: string) => Promise<BraindumpDto>,
	getGraphDelta: (since?: number) => Promise<GraphDelta>
) {
	return { submitBraindump, getGraphDelta };
}

describe('createIngestApi - POST /braindumps then GET /graph/delta → optimistic-merge payload (ADR-0002)', () => {
	it('submits the verbatim, fetches the delta since the cursor, and packages concepts/edges + fresh cursor', async () => {
		const submitBraindump = vi.fn(async () => BRAINDUMP);
		const getGraphDelta = vi.fn(async () => DELTA);
		const ingest: IngestApi = createIngestApi(
			clientStub(submitBraindump, getGraphDelta),
			() => 1_780
		);

		const res: IngestResponse = await ingest.ingest('caffeine disrupts sleep');

		expect(submitBraindump).toHaveBeenCalledWith('caffeine disrupts sleep');
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
				vi.fn(async () => DELTA)
			),
			() => 0
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
		const ingest = createIngestApi(
			clientStub(submitBraindump, getGraphDelta),
			() => 1_780
		);
		await expect(ingest.ingest('x')).rejects.toThrow(/400/);
		expect(getGraphDelta).not.toHaveBeenCalled();
	});

	it('surfaces a delta-fetch failure so the caller can fall back to the next focus event', async () => {
		const getGraphDelta = vi.fn(async () => {
			throw new Error('GET /graph/delta failed: 503');
		});
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta
			),
			() => 1_780
		);
		await expect(ingest.ingest('x')).rejects.toThrow(/503/);
	});

	it('reads the cursor lazily at ingest time so a post-submit cursor bump is seen on the next call', async () => {
		let cursor = 1_780;
		const getGraphDelta = vi.fn(async () => ({ ...DELTA, cursor: 1_900 }));
		const ingest = createIngestApi(
			clientStub(
				vi.fn(async () => BRAINDUMP),
				getGraphDelta
			),
			() => cursor
		);
		await ingest.ingest('first');
		cursor = 1_900;
		await ingest.ingest('second');
		expect(getGraphDelta).toHaveBeenNthCalledWith(2, 1_900);
	});
});
