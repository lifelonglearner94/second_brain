// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, cleanup, waitFor } from '@testing-library/svelte';

const mockHousekeeping = vi.hoisted(() => ({
	status: 'idle' as 'idle' | 'loading' | 'loaded' | 'error',
	items: [] as {
		id: number;
		kind: string;
		leftLabel: string;
		rightLabel: string;
		similarity: number;
	}[],
	load: vi.fn(async () => {})
}));

const focusCb = vi.hoisted(() => ({ fn: null as null | (() => void) }));

vi.mock('$lib/state/housekeeping.svelte', () => ({
	housekeeping: mockHousekeeping
}));

vi.mock('$lib/api', () => ({ apiClient: {} }));

vi.mock('$lib/state/graph.svelte', () => ({
	graphStore: {
		snapshot: { concepts: [], edges: [], partitions: [] },
		data: { nodes: [], links: [] },
		loadFromNetworkOrCache: vi.fn(async () => ({
			source: 'network',
			fetchedAt: '2026-07-10T00:00:00Z'
		})),
		syncDelta: vi.fn(async () => ({ applied: false }))
	}
}));

vi.mock('$lib/state/idb', () => ({ createIdb: () => ({}) }));

vi.mock('$lib/state/viewport', () => ({
	loadViewport: () => null,
	saveViewport: () => {}
}));

vi.mock('$lib/graph/frozen-graph', () => ({
	frozenGraphStatus: () => ({ status: 'ready', label: null })
}));

vi.mock('$lib/graph/build', () => ({
	buildSpatialViewGraph: () => ({ hasNode: () => false })
}));

vi.mock('$lib/graph/capability', () => ({
	detectRendererCapability: () => '2d' as const,
	probeRendererCapability: () => ({})
}));

vi.mock('$lib/graph/render2d', () => ({
	renderSpatialViewGraph2D: vi.fn(async () => ({
		destroy: vi.fn(),
		setSelected: vi.fn()
	}))
}));

vi.mock('$lib/graph/delta-sync', () => ({
	onWindowFocus: (_target: unknown, cb: () => void) => {
		focusCb.fn = cb;
		return () => {
			focusCb.fn = null;
		};
	}
}));

import GraphPage from '../../src/routes/app/graph/+page.svelte';

const ITEM = {
	id: 11,
	kind: 'concept',
	leftLabel: 'Apples',
	rightLabel: 'sleep',
	similarity: 0.92
};

describe('Spatial View-Graph - housekeeping alert banner (issue #88)', () => {
	beforeEach(() => {
		cleanup();
		mockHousekeeping.status = 'loaded';
		mockHousekeeping.items = [];
		mockHousekeeping.load.mockClear();
		focusCb.fn = null;
	});

	afterEach(() => {
		cleanup();
	});

	it('shows the banner with a count and a /app/housekeeping link when the queue is non-empty', async () => {
		mockHousekeeping.items = [ITEM, { ...ITEM, id: 33, kind: 'type' }];
		const { getByTestId } = render(GraphPage);

		const banner = await waitFor(() => getByTestId('housekeeping-banner'));
		expect(banner.textContent).toContain('2 concepts to resolve');

		const link = getByTestId('housekeeping-banner-link') as HTMLAnchorElement;
		expect(link.getAttribute('href')).toBe('/app/housekeeping');
	});

	it('hides the banner when the Housekeeping Queue is empty', () => {
		mockHousekeeping.items = [];
		const { queryByTestId } = render(GraphPage);

		expect(queryByTestId('housekeeping-banner')).toBeNull();
	});

	it('hides the banner while the housekeeping store is still loading', () => {
		mockHousekeeping.status = 'loading';
		mockHousekeeping.items = [ITEM];
		const { queryByTestId } = render(GraphPage);

		expect(queryByTestId('housekeeping-banner')).toBeNull();
	});

	it('fetches the housekeeping count on mount', async () => {
		render(GraphPage);

		await waitFor(() => expect(mockHousekeeping.load).toHaveBeenCalled());
	});

	it('re-fetches the housekeeping count when the window regains focus', async () => {
		render(GraphPage);
		await waitFor(() => expect(mockHousekeeping.load).toHaveBeenCalled());

		mockHousekeeping.load.mockClear();
		focusCb.fn?.();
		await waitFor(() => expect(mockHousekeeping.load).toHaveBeenCalled());
	});
});
