import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient, type Health, type GlobalTopologySnapshot } from '../../src/lib/api/client';

function okResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'content-type': 'application/json' }
	});
}

describe('apiClient', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('defaults the base URL to /api (same-origin Edge proxy)', async () => {
		fetchMock.mockResolvedValue(okResponse({ ok: true, db: true, sqlite_vec: true }));
		const api = createApiClient({ fetch: fetchMock });
		await api.getHealth();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/health');
		expect(init?.method).toBeUndefined();
	});

	it('uses a configured base URL', async () => {
		fetchMock.mockResolvedValue(okResponse({ ok: true, db: true, sqlite_vec: true }));
		const api = createApiClient({ baseUrl: 'https://brain.example.test', fetch: fetchMock });
		await api.getHealth();
		expect(fetchMock.mock.calls[0][0]).toBe('https://brain.example.test/health');
	});

	it('sends credentials so the opaque session cookie (backend #2) is included', async () => {
		fetchMock.mockResolvedValue(okResponse({ ok: true, db: true, sqlite_vec: true }));
		const api = createApiClient({ fetch: fetchMock });
		await api.getHealth();
		expect(fetchMock.mock.calls[0][1]).toMatchObject({ credentials: 'include' });
	});

	it('parses the GET /health body from backend #1', async () => {
		const body: Health = { ok: true, db: true, sqlite_vec: true };
		fetchMock.mockResolvedValue(okResponse(body));
		const api = createApiClient({ fetch: fetchMock });
		const health = await api.getHealth();
		expect(health).toEqual({ ok: true, db: true, sqlite_vec: true });
	});

	it('throws on a non-2xx response', async () => {
		fetchMock.mockResolvedValue(new Response('nope', { status: 503 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getHealth()).rejects.toThrow(/503/);
	});
});

describe('apiClient.getGraph — Global Topology Snapshot fetch (#16, backend #27)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

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
			{ concept_id: 'c2', partition_id: 1 }
		]
	};

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /graph and parses concepts, typed edges, and Louvain partition IDs', async () => {
		fetchMock.mockResolvedValue(okResponse(SNAPSHOT));
		const api = createApiClient({ fetch: fetchMock });
		const graph = await api.getGraph();
		expect(graph).toEqual(SNAPSHOT);
		expect(graph.concepts).toHaveLength(2);
		expect(graph.edges[0]?.current_type).toBe('affects');
		expect(graph.partitions[0]?.partition_id).toBe(0);
	});

	it('hits the /graph path under the configured base URL', async () => {
		fetchMock.mockResolvedValue(okResponse(SNAPSHOT));
		const api = createApiClient({ baseUrl: 'https://brain.example.test', fetch: fetchMock });
		await api.getGraph();
		expect(fetchMock.mock.calls[0][0]).toBe('https://brain.example.test/graph');
	});

	it('sends credentials so the opaque session cookie reaches the authed endpoint (#15)', async () => {
		fetchMock.mockResolvedValue(okResponse(SNAPSHOT));
		const api = createApiClient({ fetch: fetchMock });
		await api.getGraph();
		expect(fetchMock.mock.calls[0][1]).toMatchObject({ credentials: 'include' });
	});

	it('accepts the gzip-transparent JSON the backend returns (Content-Encoding is decompressed by fetch)', async () => {
		fetchMock.mockResolvedValue(
			new Response(JSON.stringify(SNAPSHOT), {
				status: 200,
				headers: { 'content-type': 'application/json', 'content-encoding': 'gzip' }
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		const graph = await api.getGraph();
		expect(graph.concepts).toHaveLength(2);
	});

	it('throws on a non-2xx so the view can fall back to the IDB Frozen Graph cache', async () => {
		fetchMock.mockResolvedValue(new Response('nope', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getGraph()).rejects.toThrow(/401/);
	});
});
