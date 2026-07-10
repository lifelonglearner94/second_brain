// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, cleanup, waitFor } from '@testing-library/svelte';
import HousekeepingQueue from '../../src/lib/housekeeping/HousekeepingQueue.svelte';
import {
	HousekeepingStore,
	type HousekeepingApi
} from '../../src/lib/state/housekeeping.svelte';
import { GraphStore } from '../../src/lib/state/graph.svelte';
import type {
	GlobalTopologySnapshot,
	ConceptMergeSuggestion,
	Ontology,
	OntologyTypeProposal,
	OntologyProposalsResponse
} from '../../src/lib/api/client';

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: '1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: '2', label: 'Apples', created_at: '2026-07-03T00:00:00Z' }
	],
	edges: [],
	partitions: [
		{ concept_id: '1', partition_id: 0 },
		{ concept_id: '2', partition_id: 1 }
	]
};

const ONTOLOGY: Ontology = {
	edge_types: [
		{ slug: 'affects', label: 'Affects', description: 'Has an effect on.' }
	]
};

const CONCEPT_SUGGESTION: ConceptMergeSuggestion = {
	id: 11,
	kind: 'concept',
	braindump_id: 5,
	new_concept_label: 'Apples',
	new_concept_id: 2,
	existing_concept_id: 1,
	existing_concept_label: 'sleep',
	braindump_snippet: 'I had apples today and they affected my sleep.',
	similarity: 0.92,
	status: 'pending',
	created_at: 1_700_000_000
};

const TYPE_PROPOSAL: OntologyTypeProposal = {
	id: 33,
	slug: 'endangers',
	label: 'Endangers',
	description: 'Causes harm to.',
	merge_of: 'affects',
	status: 'pending',
	near_match_slug: 'affects',
	near_match_similarity: 0.88
};

function apiStub(overrides: Partial<HousekeepingApi> = {}): HousekeepingApi {
	return {
		getMergeSuggestions: vi.fn(async () => [CONCEPT_SUGGESTION]),
		approveMergeSuggestion: vi.fn(async () => undefined),
		rejectMergeSuggestion: vi.fn(async () => undefined),
		getOntology: vi.fn(async () => ONTOLOGY),
		getOntologyProposals: vi.fn(
			async () =>
				({ proposals: [TYPE_PROPOSAL] }) satisfies OntologyProposalsResponse
		),
		approveOntologyProposal: vi.fn(async () => ({
			...TYPE_PROPOSAL,
			status: 'approved'
		})),
		rejectOntologyProposal: vi.fn(async () => ({
			...TYPE_PROPOSAL,
			status: 'rejected'
		})),
		...overrides
	};
}

async function makeLoadedStore(
	api: HousekeepingApi
): Promise<HousekeepingStore> {
	const graph = new GraphStore();
	graph.loadSnapshot(SNAPSHOT);
	const store = new HousekeepingStore(api, graph);
	await store.load();
	return store;
}

describe('HousekeepingQueue.svelte - the low-epistemic-weight HITL surface (ADR-0004)', () => {
	beforeEach(() => {
		cleanup();
	});
	afterEach(() => {
		cleanup();
	});

	it('lists both the concept- and type-merge suggestion with their borderline pair and similarity score', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { getByText, container } = render(HousekeepingQueue, {
			props: { store }
		});

		expect(getByText('Apples')).toBeTruthy();
		expect(getByText('sleep')).toBeTruthy();
		expect(getByText(/Similarity 0\.92/)).toBeTruthy();
		expect(getByText('Endangers')).toBeTruthy();
		expect(getByText('Affects')).toBeTruthy();
		expect(getByText(/Similarity 0\.88/)).toBeTruthy();

		expect(
			container.querySelectorAll('[data-testid^="housekeeping-item-"]')
		).toHaveLength(2);
	});

	it('uses the action verb "Merge" on every suggestion (distinct from the Endorsement Queue "Approve Connection")', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { getAllByRole, queryByText } = render(HousekeepingQueue, {
			props: { store }
		});

		const mergeButtons = getAllByRole('button', { name: 'Merge' });
		expect(mergeButtons).toHaveLength(2);
		expect(queryByText('Approve Connection')).toBeNull();
	});

	it('shows NO Evidence Disclosure (only similarity scores - that lives in the Endorsement Queue)', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { queryByText } = render(HousekeepingQueue, { props: { store } });

		expect(queryByText(/Based on existing path/)).toBeNull();
		expect(queryByText(/Based on thematic density/)).toBeNull();
		expect(queryByText(/evidence/i)).toBeNull();
	});

	it('tags each kind so the bifurcation between concept and type merges is visible', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { getByTestId } = render(HousekeepingQueue, { props: { store } });

		expect(getByTestId('housekeeping-item-11-concept')).toBeTruthy();
		expect(getByTestId('housekeeping-item-33-type')).toBeTruthy();
	});

	it('confirming a concept merge POSTs and optimistically removes it from the queue', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { getByTestId, queryByText } = render(HousekeepingQueue, {
			props: { store }
		});

		await fireEvent.click(getByTestId('housekeeping-merge-11-concept'));

		expect(api.approveMergeSuggestion).toHaveBeenCalledWith(11);
		await waitFor(() => {
			expect(queryByText('Apples')).toBeNull();
		});
	});

	it('confirming a type merge POSTs and optimistically removes it from the queue', async () => {
		const api = apiStub();
		const store = await makeLoadedStore(api);
		const { getByTestId, queryByText } = render(HousekeepingQueue, {
			props: { store }
		});

		await fireEvent.click(getByTestId('housekeeping-merge-33-type'));

		expect(api.approveOntologyProposal).toHaveBeenCalledWith(33);
		await waitFor(() => {
			expect(queryByText('Endangers')).toBeNull();
		});
	});

	it('renders an empty state when the queue has no suggestions (nothing pending)', async () => {
		const api = apiStub({
			getMergeSuggestions: vi.fn(async () => []),
			getOntologyProposals: vi.fn(
				async () => ({ proposals: [] }) satisfies OntologyProposalsResponse
			)
		});
		const store = await makeLoadedStore(api);
		const { getByTestId, queryByRole } = render(HousekeepingQueue, {
			props: { store }
		});

		expect(getByTestId('housekeeping-empty')).toBeTruthy();
		expect(queryByRole('button', { name: 'Merge' })).toBeNull();
	});

	it('renders a loading state before the first load completes', async () => {
		const api = apiStub();
		const store = new HousekeepingStore(api, new GraphStore());
		const { getByTestId } = render(HousekeepingQueue, { props: { store } });

		expect(getByTestId('housekeeping-loading')).toBeTruthy();
	});

	it('renders a load error without fabricating suggestions', async () => {
		const api = apiStub({
			getMergeSuggestions: vi.fn(async () => {
				throw new Error('GET /merge-suggestions failed: 401');
			})
		});
		const store = await makeLoadedStore(api);
		const { getByTestId, queryByRole } = render(HousekeepingQueue, {
			props: { store }
		});

		expect(getByTestId('housekeeping-error')).toBeTruthy();
		expect(queryByRole('button', { name: 'Merge' })).toBeNull();
	});

	describe('Keep separate - the low-epistemic-weight reject action (ADR-0004)', () => {
		it('renders a "Keep separate" button for concept items', async () => {
			const api = apiStub();
			const store = await makeLoadedStore(api);
			const { getByTestId } = render(HousekeepingQueue, {
				props: { store }
			});

			expect(getByTestId('housekeeping-reject-11-concept')).toBeTruthy();
		});

		it('clicking "Keep separate" calls store.rejectMerge and removes the item', async () => {
			const api = apiStub();
			const store = await makeLoadedStore(api);
			const { getByTestId, queryByText } = render(HousekeepingQueue, {
				props: { store }
			});

			await fireEvent.click(getByTestId('housekeeping-reject-11-concept'));

			expect(api.rejectMergeSuggestion).toHaveBeenCalledWith(11);
			await waitFor(() => {
				expect(queryByText('Apples')).toBeNull();
			});
		});
	});

	describe('provenance - the braindump snippet that triggered the suggestion', () => {
		it('shows the provenance text when braindumpSnippet is present', async () => {
			const api = apiStub();
			const store = await makeLoadedStore(api);
			const { getByTestId } = render(HousekeepingQueue, {
				props: { store }
			});

			expect(getByTestId('housekeeping-provenance-11-concept')).toBeTruthy();
			expect(
				getByTestId('housekeeping-provenance-11-concept').textContent
			).toContain('I had apples today and they affected my sleep.');
		});

		it('hides the provenance text when braindumpSnippet is null (type items)', async () => {
			const api = apiStub();
			const store = await makeLoadedStore(api);
			const { queryByTestId } = render(HousekeepingQueue, {
				props: { store }
			});

			expect(queryByTestId('housekeeping-provenance-33-type')).toBeNull();
		});
	});
});
