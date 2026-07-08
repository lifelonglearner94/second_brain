import { describe, it, expect, vi } from 'vitest';
import {
	AdminSystemStore,
	type AdminSystemApi
} from '../../src/lib/state/admin-system.svelte';
import type { SystemResponse } from '../../src/lib/api/client';

const METRICS: SystemResponse = {
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

function apiStub(getSystem: AdminSystemApi['getSystem']): AdminSystemApi {
	return { getSystem };
}

describe('AdminSystemStore — host load surface over backend #81 GET /admin/system', () => {
	it('starts idle with no metrics', () => {
		const store = new AdminSystemStore(
			apiStub(vi.fn<AdminSystemApi['getSystem']>())
		);
		expect(store.status).toBe('idle');
		expect(store.metrics).toBeNull();
		expect(store.error).toBeNull();
	});

	it('refresh() loads metrics from the backend and flips to loaded', async () => {
		const getSystem = vi
			.fn<AdminSystemApi['getSystem']>()
			.mockResolvedValue(METRICS);
		const store = new AdminSystemStore(apiStub(getSystem));
		await store.refresh();
		expect(getSystem).toHaveBeenCalledOnce();
		expect(store.status).toBe('loaded');
		expect(store.metrics).toEqual(METRICS);
		expect(store.error).toBeNull();
	});

	it('refresh() replaces the previous snapshot on re-fetch (no stale merge)', async () => {
		const getSystem = vi
			.fn<AdminSystemApi['getSystem']>()
			.mockResolvedValueOnce(METRICS)
			.mockResolvedValueOnce({
				...METRICS,
				cpu: { usage_percent: 90.0, cores: 2, per_core: [95.0, 85.0] }
			});
		const store = new AdminSystemStore(apiStub(getSystem));
		await store.refresh();
		expect(store.metrics?.cpu.usage_percent).toBe(12.5);
		await store.refresh();
		expect(store.metrics?.cpu.usage_percent).toBe(90.0);
	});

	it('refresh() flips to error and surfaces the message when the fetch rejects (e.g. 401)', async () => {
		const getSystem = vi
			.fn<AdminSystemApi['getSystem']>()
			.mockRejectedValue(new Error('GET /admin/system failed: 401'));
		const store = new AdminSystemStore(apiStub(getSystem));
		await store.refresh();
		expect(store.status).toBe('error');
		expect(store.error).toMatch(/401/);
		expect(store.metrics).toBeNull();
	});
});
