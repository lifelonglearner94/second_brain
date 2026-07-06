import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
	createApiClient,
	type BraindumpDto,
	type GraphDelta
} from '../../src/lib/api/client';

function okResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'content-type': 'application/json' }
	});
}

describe('apiClient.submitBraindump — POST /braindumps (backend #5, ADR-0007)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs { verbatim } to /braindumps with credentials so the session cookie reaches the authed write path', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				id: 7,
				verbatim: 'hallo',
				cleaned: 'Hallo.',
				created_at: 1_780
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		await api.submitBraindump('hallo');
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/braindumps');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
		expect(JSON.parse(init?.body as string)).toEqual({ verbatim: 'hallo' });
	});

	it('parses the Braindump body (id + verbatim + cleaned + created_at)', async () => {
		const body: BraindumpDto = {
			id: '7',
			verbatim: 'hallo welt',
			cleaned: 'Hallo, Welt.',
			created_at: '1780'
		};
		fetchMock.mockResolvedValue(okResponse(body));
		const api = createApiClient({ fetch: fetchMock });
		const braindump = await api.submitBraindump('hallo welt');
		expect(braindump).toEqual(body);
	});

	it('throws on a non-2xx so the Active Capture submit can surface the error', async () => {
		fetchMock.mockResolvedValue(new Response('nope', { status: 400 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.submitBraindump('x')).rejects.toThrow(/400/);
	});
});

describe('apiClient.getGraphDelta — GET /graph/delta (backend #28, ADR-0002 pull-on-focus)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

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

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /graph/delta with the cursor as the since query param', async () => {
		fetchMock.mockResolvedValue(okResponse(DELTA));
		const api = createApiClient({ fetch: fetchMock });
		await api.getGraphDelta(1_780);
		expect(fetchMock.mock.calls[0][0]).toBe('/api/graph/delta?since=1780');
		expect(fetchMock.mock.calls[0][1]).toMatchObject({
			credentials: 'include'
		});
	});

	it('GETs /graph/delta with no query param when no cursor is supplied (first sync)', async () => {
		fetchMock.mockResolvedValue(okResponse(DELTA));
		const api = createApiClient({ fetch: fetchMock });
		await api.getGraphDelta();
		expect(fetchMock.mock.calls[0][0]).toBe('/api/graph/delta');
	});

	it('parses added_concepts, added_edges, deletions, retags, and the fresh cursor', async () => {
		fetchMock.mockResolvedValue(okResponse(DELTA));
		const api = createApiClient({ fetch: fetchMock });
		const delta = await api.getGraphDelta(1_780);
		expect(delta.cursor).toBe(1_800);
		expect(delta.added_concepts[0]?.label).toBe('caffeine');
		expect(delta.added_edges[0]?.current_type).toBe('disrupts');
		expect(delta.deleted_concept_ids).toEqual([]);
		expect(delta.retagged_edges).toEqual([]);
	});

	it('throws on a non-2xx so the optimistic merge can fall back to the next focus event', async () => {
		fetchMock.mockResolvedValue(new Response('nope', { status: 503 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getGraphDelta(1_780)).rejects.toThrow(/503/);
	});
});
