// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

const mockGraphStore = vi.hoisted(() => ({
	snapshot: { concepts: [], edges: [], partitions: [] } as unknown,
	cursor: 1700000000,
	data: { nodes: [], links: [] },
	syncDelta: vi.fn(async () => ({ applied: true })),
	mergeIngest: vi.fn(),
	loadFromNetworkOrCache: vi.fn(async () => ({
		source: 'network',
		fetchedAt: '2026-07-17T00:00:00Z'
	}))
}));

const sessionStub = vi.hoisted(() => ({
	userId: 'u1',
	status: 'authenticated' as const,
	clear: vi.fn()
}));

const pendingStub = vi.hoisted(() => ({
	count: 0,
	items: [] as unknown[],
	load: vi.fn(async () => {})
}));

const apiStub = vi.hoisted(() => ({
	logout: vi.fn(async () => {}),
	getGraphDelta: vi.fn(async () => ({
		cursor: 1700000500,
		added_concepts: [],
		added_edges: [],
		deleted_concept_ids: [],
		deleted_edge_ids: [],
		retagged_edges: []
	})),
	submitBraindump: vi.fn(),
	getIngestStatus: vi.fn()
}));

vi.mock('$app/navigation', () => ({ goto: vi.fn() }));
vi.mock('$lib/api', () => ({ apiClient: apiStub }));
vi.mock('$lib/state/session.svelte', () => ({ session: sessionStub }));
vi.mock('$lib/state/pending-captures.svelte', () => ({
	pendingCaptures: pendingStub
}));
vi.mock('$lib/state/graph.svelte', () => ({ graphStore: mockGraphStore }));
vi.mock('$lib/state/idb', () => ({ createIdb: () => ({}) }));

import AppPage from '../../src/routes/app/+page.svelte';

describe('/app page - focus-triggered delta-sync for slow backend ingests (issue #98)', () => {
	beforeEach(() => {
		cleanup();
		mockGraphStore.snapshot = {
			concepts: [],
			edges: [],
			partitions: []
		};
		mockGraphStore.cursor = 1700000000;
		mockGraphStore.syncDelta.mockClear();
		mockGraphStore.mergeIngest.mockClear();
		mockGraphStore.loadFromNetworkOrCache.mockClear();
		pendingStub.load.mockClear();
	});
	afterEach(() => {
		cleanup();
	});

	it('bootstraps the canonical snapshot on mount so a hard-reload surfaces late commits', async () => {
		render(AppPage);

		await waitFor(() =>
			expect(mockGraphStore.loadFromNetworkOrCache).toHaveBeenCalledOnce()
		);
	});

	it('wires onWindowFocus so a window focus triggers graphStore.syncDelta', async () => {
		render(AppPage);
		await waitFor(() =>
			expect(mockGraphStore.loadFromNetworkOrCache).toHaveBeenCalled()
		);

		window.dispatchEvent(new Event('focus'));

		await waitFor(() =>
			expect(mockGraphStore.syncDelta).toHaveBeenCalledOnce()
		);
	});

	it('does not call syncDelta on focus when the store has no snapshot loaded (cursor-advance guard from #97)', async () => {
		mockGraphStore.snapshot = null;
		render(AppPage);
		await waitFor(() =>
			expect(mockGraphStore.loadFromNetworkOrCache).toHaveBeenCalled()
		);

		window.dispatchEvent(new Event('focus'));
		await new Promise((r) => setTimeout(r, 0));

		expect(mockGraphStore.syncDelta).not.toHaveBeenCalled();
	});

	it('removes the focus listener on unmount so no sync fires after cleanup (no double-sync on /app/graph)', async () => {
		render(AppPage);
		await waitFor(() =>
			expect(mockGraphStore.loadFromNetworkOrCache).toHaveBeenCalled()
		);
		window.dispatchEvent(new Event('focus'));
		await waitFor(() =>
			expect(mockGraphStore.syncDelta).toHaveBeenCalledOnce()
		);

		cleanup();
		mockGraphStore.syncDelta.mockClear();
		window.dispatchEvent(new Event('focus'));
		await new Promise((r) => setTimeout(r, 0));

		expect(mockGraphStore.syncDelta).not.toHaveBeenCalled();
	});
});
