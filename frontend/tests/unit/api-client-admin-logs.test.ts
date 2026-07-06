import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient, type LogsResponse } from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const LOGS_BODY: LogsResponse = {
	logs: [
		{
			timestamp: 1_700_000_000,
			level: 'ERROR',
			target: 'gemini_client',
			message: 'generation failed',
			fields: { status: 503, retries: 3 }
		},
		{
			timestamp: 1_700_000_010,
			level: 'WARN',
			target: 'gemini_client',
			message: 'retrying',
			fields: { attempt: 1 }
		}
	],
	count: 2,
	capacity: 1_000
};

describe('apiClient — admin logs surface against backend #4', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GET /admin/logs is credentialed so the auth cookie (backend #2) is sent', async () => {
		fetchMock.mockResolvedValue(okResponse(LOGS_BODY));
		const api = createApiClient({ fetch: fetchMock });
		await api.getAdminLogs();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/admin/logs');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
	});

	it('omits the query when no limit is given (backend defaults to 200)', async () => {
		fetchMock.mockResolvedValue(okResponse(LOGS_BODY));
		const api = createApiClient({ fetch: fetchMock });
		await api.getAdminLogs();
		expect(fetchMock.mock.calls[0][0]).toBe('/api/admin/logs');
	});

	it('forwards ?limit=N when a limit is supplied (capped server-side at capacity)', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				logs: LOGS_BODY.logs.slice(0, 1),
				count: 1,
				capacity: 1_000
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		await api.getAdminLogs(50);
		expect(fetchMock.mock.calls[0][0]).toBe('/api/admin/logs?limit=50');
	});

	it('parses the { logs, count, capacity } body from backend #4', async () => {
		fetchMock.mockResolvedValue(okResponse(LOGS_BODY));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.getAdminLogs();
		expect(res).toEqual(LOGS_BODY);
		expect(res.count).toBe(2);
		expect(res.capacity).toBe(1_000);
		expect(res.logs[0].fields).toEqual({ status: 503, retries: 3 });
	});

	it('throws on a non-2xx response (401 when no session)', async () => {
		fetchMock.mockResolvedValue(new Response('unauthorized', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getAdminLogs()).rejects.toThrow(/401/);
	});
});
