import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient, type Health } from '../../src/lib/api/client';

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
