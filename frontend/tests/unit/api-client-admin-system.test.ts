import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient, type SystemResponse } from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const SYSTEM_BODY: SystemResponse = {
	cpu: {
		usage_percent: 12.5,
		cores: 2,
		per_core: [10.0, 15.0]
	},
	memory: {
		total_bytes: 8_589_934_592,
		used_bytes: 2_147_483_648,
		usage_percent: 25.0
	},
	disks: [
		{
			name: '/',
			mount_point: '/',
			total_bytes: 536_870_912_000,
			used_bytes: 268_435_456_000,
			usage_percent: 50.0
		}
	],
	brain_file_mount: '/'
};

describe('apiClient - admin system surface against backend #81', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GET /admin/system is credentialed so the auth cookie is sent', async () => {
		fetchMock.mockResolvedValue(okResponse(SYSTEM_BODY));
		const api = createApiClient({ fetch: fetchMock });
		await api.getSystem();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/admin/system');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
	});

	it('parses the { cpu, memory, disks, brain_file_mount } body from backend #81', async () => {
		fetchMock.mockResolvedValue(okResponse(SYSTEM_BODY));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.getSystem();
		expect(res).toEqual(SYSTEM_BODY);
		expect(res.cpu.cores).toBe(2);
		expect(res.cpu.per_core).toHaveLength(2);
		expect(res.memory.total_bytes).toBe(8_589_934_592);
		expect(res.disks[0].mount_point).toBe('/');
		expect(res.brain_file_mount).toBe('/');
	});

	it('surfaces a null brain_file_mount when the db is in-memory', async () => {
		fetchMock.mockResolvedValue(
			okResponse({ ...SYSTEM_BODY, brain_file_mount: null })
		);
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.getSystem();
		expect(res.brain_file_mount).toBeNull();
	});

	it('throws on a non-2xx response (401 when no session)', async () => {
		fetchMock.mockResolvedValue(new Response('unauthorized', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getSystem()).rejects.toThrow(/401/);
	});
});
