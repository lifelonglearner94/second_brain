import { describe, it, expect, vi } from 'vitest';
import { AdminLogStore, type AdminLogApi } from '../../src/lib/state/admin-logs.svelte';
import type { LogsResponse } from '../../src/lib/api/client';

const LOGS: LogsResponse = {
	logs: [
		{
			timestamp: 1_700_000_000,
			level: 'ERROR',
			target: 'gemini_client',
			message: 'generation failed',
			fields: { status: 503 }
		},
		{
			timestamp: 1_700_000_010,
			level: 'WARN',
			target: 'gemini_client',
			message: 'retrying',
			fields: { attempt: 1 }
		},
		{
			timestamp: 1_700_000_020,
			level: 'INFO',
			target: 'ingest',
			message: 'braindump accepted',
			fields: { id: 'b1' }
		}
	],
	count: 3,
	capacity: 1_000
};

function apiStub(getAdminLogs: AdminLogApi['getAdminLogs']): AdminLogApi {
	return { getAdminLogs };
}

describe('AdminLogStore — pull-based log surface over backend #4 GET /admin/logs', () => {
	it('starts idle with an empty, bounded surface (no fabricated logs)', () => {
		const store = new AdminLogStore(apiStub(vi.fn<AdminLogApi['getAdminLogs']>()));
		expect(store.status).toBe('idle');
		expect(store.logs).toEqual([]);
		expect(store.count).toBe(0);
		expect(store.capacity).toBe(0);
		expect(store.filtered).toEqual([]);
	});

	it('refresh() loads logs/count/capacity from the backend and flips to loaded', async () => {
		const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
		const store = new AdminLogStore(apiStub(getAdminLogs));
		await store.refresh();
		expect(getAdminLogs).toHaveBeenCalledOnce();
		expect(store.status).toBe('loaded');
		expect(store.logs).toEqual(LOGS.logs);
		expect(store.count).toBe(3);
		expect(store.capacity).toBe(1_000);
		expect(store.filtered).toEqual(LOGS.logs);
	});

	it('refresh() forwards a limit to the API (capped server-side at capacity)', async () => {
		const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue({
			logs: LOGS.logs.slice(0, 1),
			count: 1,
			capacity: 1_000
		});
		const store = new AdminLogStore(apiStub(getAdminLogs));
		await store.refresh(50);
		expect(getAdminLogs).toHaveBeenCalledWith(50);
	});

	it('refresh() flips to error and surfaces the message when the fetch rejects (e.g. 401)', async () => {
		const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockRejectedValue(new Error('GET /admin/logs failed: 401'));
		const store = new AdminLogStore(apiStub(getAdminLogs));
		await store.refresh();
		expect(store.status).toBe('error');
		expect(store.error).toMatch(/401/);
		expect(store.logs).toEqual([]);
	});

	it('filtered is bounded by what the backend returned — nothing is synthesised client-side', async () => {
		const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
		const store = new AdminLogStore(apiStub(getAdminLogs));
		await store.refresh();
		expect(store.filtered.length).toBe(3);
		expect(store.filtered.length).toBeLessThanOrEqual(store.count);
	});

	describe('level filter', () => {
		it('defaults to "all" (no level filter) so every returned log is visible', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			expect(store.levelFilter).toBe('all');
			expect(store.filtered).toHaveLength(3);
		});

		it('levels reflects exactly the levels the backend returned (no hard-coded list)', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			expect(store.levels.sort()).toEqual(['ERROR', 'INFO', 'WARN']);
		});

		it('setting the level filter narrows filtered to that level only', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.levelFilter = 'WARN';
			expect(store.filtered).toHaveLength(1);
			expect(store.filtered[0].level).toBe('WARN');
		});

		it('resetting to "all" restores the full bounded set', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.levelFilter = 'ERROR';
			store.levelFilter = 'all';
			expect(store.filtered).toHaveLength(3);
		});
	});

	describe('text search', () => {
		it('matches a substring of the message (case-insensitive)', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.query = 'FAILED';
			expect(store.filtered).toHaveLength(1);
			expect(store.filtered[0].message).toBe('generation failed');
		});

		it('matches a substring of the target module', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.query = 'ingest';
			expect(store.filtered).toHaveLength(1);
			expect(store.filtered[0].target).toBe('ingest');
		});

		it('matches a substring of the structured fields JSON', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.query = '503';
			expect(store.filtered).toHaveLength(1);
			expect(store.filtered[0].level).toBe('ERROR');
		});

		it('combines level filter and text search (AND semantics)', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.levelFilter = 'WARN';
			store.query = 'gemini';
			expect(store.filtered).toHaveLength(1);
			expect(store.filtered[0].level).toBe('WARN');
			store.query = 'ingest';
			expect(store.filtered).toHaveLength(0);
		});

		it('a blank query is treated as no search', async () => {
			const getAdminLogs = vi.fn<AdminLogApi['getAdminLogs']>().mockResolvedValue(LOGS);
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			store.query = '   ';
			expect(store.filtered).toHaveLength(3);
		});
	});

	describe('empty surface', () => {
		it('refresh() on an empty buffer renders nothing (loaded, zero logs)', async () => {
			const getAdminLogs = vi
				.fn<AdminLogApi['getAdminLogs']>()
				.mockResolvedValue({ logs: [], count: 0, capacity: 1_000 });
			const store = new AdminLogStore(apiStub(getAdminLogs));
			await store.refresh();
			expect(store.status).toBe('loaded');
			expect(store.logs).toEqual([]);
			expect(store.filtered).toEqual([]);
			expect(store.levels).toEqual([]);
		});
	});
});
