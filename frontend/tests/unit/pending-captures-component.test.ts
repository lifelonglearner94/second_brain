// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/svelte';
import PendingCaptures from '../../src/lib/capture/PendingCaptures.svelte';
import { PendingCapturesStore } from '../../src/lib/state/pending-captures.svelte';
import type { IdbStore, PendingCapture } from '../../src/lib/state/idb';
import type { IngestApi, IngestResponse } from '../../src/lib/capture/ingest';
import { applyDelta } from '../../src/lib/graph/delta';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

const CAPTURE_A: PendingCapture = {
	id: 'a',
	text: 'offline thought one',
	createdAt: '2026-07-04T01:00:00Z'
};

const CAPTURE_B: PendingCapture = {
	id: 'b',
	text: 'offline thought two',
	createdAt: '2026-07-04T02:00:00Z'
};

const INGESTED: IngestResponse = {
	braindump: { id: 'b1', created_at: '200' },
	concepts: [{ id: 'c2', label: 'caffeine', created_at: '200' }],
	edges: [
		{
			id: 'e1',
			source_concept_id: 'c2',
			target_concept_id: 'c1',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '200'
		}
	],
	cursor: 200
};

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [{ id: 'c1', label: 'sleep', created_at: '100' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
};

function fakeIdb(): IdbStore {
	const stored: PendingCapture[] = [];
	return {
		saveTopologySnapshot: vi.fn(),
		loadTopologySnapshot: vi.fn(),
		clearTopologySnapshot: vi.fn(),
		enqueuePendingCapture: vi.fn(async (c: PendingCapture) => {
			stored.push(c);
		}),
		listPendingCaptures: vi.fn(async () => [...stored]),
		removePendingCapture: vi.fn(async (id: string) => {
			const i = stored.findIndex((c) => c.id === id);
			if (i >= 0) stored.splice(i, 1);
		})
	} as unknown as IdbStore;
}

function fakeIngest(res: IngestResponse): IngestApi & { calls: string[] } {
	const calls: string[] = [];
	return {
		calls,
		async ingest(verbatim: string) {
			calls.push(verbatim);
			return res;
		}
	};
}

function storeWith(...captures: PendingCapture[]): PendingCapturesStore {
	const store = new PendingCapturesStore(fakeIdb());
	store.items = [...captures];
	return store;
}

afterEach(() => {
	cleanup();
});

describe('PendingCaptures — review-and-confirm surface (ADR-0005/0007)', () => {
	it('surfaces every queued capture for review on reconnect', () => {
		const store = storeWith(CAPTURE_A, CAPTURE_B);
		render(PendingCaptures, {
			props: { store, ingest: fakeIngest(INGESTED), oningest: vi.fn() }
		});
		const items = screen.getAllByTestId(/^pending-capture-item-/);
		expect(items).toHaveLength(2);
		expect(screen.getByTestId('pending-capture-item-a')).toBeTruthy();
		expect(screen.getByTestId('pending-capture-item-b')).toBeTruthy();
	});

	it('renders an explicit empty state when no captures are queued', () => {
		const store = storeWith();
		render(PendingCaptures, {
			props: { store, ingest: fakeIngest(INGESTED), oningest: vi.fn() }
		});
		expect(screen.getByTestId('pending-captures-empty')).toBeTruthy();
		expect(screen.queryByTestId('pending-captures-list')).toBeNull();
	});

	it('pre-fills each row with the captured text in an editable textarea for correction', () => {
		const store = storeWith(CAPTURE_A);
		render(PendingCaptures, {
			props: { store, ingest: fakeIngest(INGESTED), oningest: vi.fn() }
		});
		const textarea = screen.getByTestId('pending-capture-text') as HTMLTextAreaElement;
		expect(textarea.value).toBe('offline thought one');
	});

	it('never auto-submits — ingest is not called until the user clicks Submit', async () => {
		const store = storeWith(CAPTURE_A);
		const ingest = fakeIngest(INGESTED);
		render(PendingCaptures, { props: { store, ingest, oningest: vi.fn() } });
		expect(ingest.calls).toHaveLength(0);
	});

	it('on explicit Submit, posts the corrected text through the #19 ingest path, removes the capture, and merges into the Spatial View-Graph', async () => {
		const store = storeWith(CAPTURE_A);
		const ingest = fakeIngest(INGESTED);
		let merged: GlobalTopologySnapshot = SNAPSHOT;
		const oningest = vi.fn((res: IngestResponse) => {
			merged = applyDelta(merged, {
				cursor: res.cursor,
				added_concepts: res.concepts,
				added_edges: res.edges,
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			});
		});
		render(PendingCaptures, { props: { store, ingest, oningest } });

		const textarea = screen.getByTestId('pending-capture-text') as HTMLTextAreaElement;
		await fireEvent.input(textarea, { target: { value: 'corrected offline thought' } });

		await fireEvent.click(screen.getByTestId('pending-capture-submit'));

		await waitFor(() => expect(ingest.calls).toEqual(['corrected offline thought']));
		expect(oningest).toHaveBeenCalledWith(INGESTED);
		expect(store.items).toHaveLength(0);
		await waitFor(() => expect(screen.getByTestId('pending-captures-empty')).toBeTruthy());
		expect(merged.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2']);
		expect(merged.edges.map((e) => e.id)).toContain('e1');
	});

	it('does not remove the capture or merge when the ingest POST rejects', async () => {
		const store = storeWith(CAPTURE_A);
		const ingest: IngestApi = {
			async ingest() {
				throw new Error('POST /ingest failed: 503');
			}
		};
		const oningest = vi.fn();
		render(PendingCaptures, { props: { store, ingest, oningest } });

		await fireEvent.click(screen.getByTestId('pending-capture-submit'));

		await waitFor(() =>
			expect(screen.getByTestId('pending-capture-error').textContent).toMatch(/503/)
		);
		expect(oningest).not.toHaveBeenCalled();
		expect(store.items).toHaveLength(1);
		expect(screen.getByTestId('pending-capture-item-a')).toBeTruthy();
	});
});
