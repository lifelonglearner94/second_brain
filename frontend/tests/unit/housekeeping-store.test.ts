import { describe, it, expect, vi, beforeEach } from 'vitest';
import { HousekeepingStore, type HousekeepingApi, type HousekeepingItem } from '../../src/lib/state/housekeeping.svelte';
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
		{ id: '2', label: 'Apples', created_at: '2026-07-03T00:00:00Z' },
		{ id: '3', label: 'caffeine', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: '2',
			target_concept_id: '1',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-03T00:00:00Z'
		},
		{
			id: 'e2',
			source_concept_id: '3',
			target_concept_id: '2',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '2026-07-03T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: '1', partition_id: 0 },
		{ concept_id: '2', partition_id: 1 },
		{ concept_id: '3', partition_id: 1 }
	]
};

const ONTOLOGY: Ontology = {
	edge_types: [
		{ slug: 'affects', label: 'Affects', description: 'Has an effect on.' },
		{ slug: 'disrupts', label: 'Disrupts', description: 'Interferes with.' }
	]
};

const CONCEPT_SUGGESTION: ConceptMergeSuggestion = {
	id: 11,
	kind: 'concept',
	braindump_id: 5,
	new_concept_label: 'Apples',
	new_concept_id: 2,
	existing_concept_id: 1,
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
		getGraph: vi.fn(async () => SNAPSHOT),
		getMergeSuggestions: vi.fn(async () => [CONCEPT_SUGGESTION]),
		approveMergeSuggestion: vi.fn(async () => undefined),
		getOntology: vi.fn(async () => ONTOLOGY),
		getOntologyProposals: vi.fn(async () => ({ proposals: [TYPE_PROPOSAL] }) satisfies OntologyProposalsResponse),
		approveOntologyProposal: vi.fn(async () => ({ ...TYPE_PROPOSAL, status: 'approved' })),
		...overrides
	};
}

describe('HousekeepingStore — the low-epistemic-weight HITL surface (ADR-0004)', () => {
	let api: HousekeepingApi;

	beforeEach(() => {
		api = apiStub();
	});

	it('starts idle with an empty queue and no Spatial View-Graph loaded', () => {
		const store = new HousekeepingStore(api);
		expect(store.status).toBe('idle');
		expect(store.items).toEqual([]);
		expect(store.snapshot).toBeNull();
	});

	describe('load — list concept- and type-merge suggestions from the backend Merge Suggestion API', () => {
		it('fetches the graph, concept suggestions, type proposals, and ontology type context', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			expect(api.getGraph).toHaveBeenCalledOnce();
			expect(api.getMergeSuggestions).toHaveBeenCalledOnce();
			expect(api.getOntologyProposals).toHaveBeenCalledOnce();
			expect(api.getOntology).toHaveBeenCalledOnce();
			expect(store.status).toBe('loaded');
		});

		it('lists both concept- and type-merge suggestions in one bifurcated queue', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const kinds = store.items.map((i) => i.kind).sort();
			expect(kinds).toEqual(['concept', 'type']);
		});

		it('shows the borderline pair and a similarity score for a concept suggestion (verb is Merge, set by the UI)', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const concept = store.items.find((i) => i.kind === 'concept') as HousekeepingItem;
			expect(concept.leftLabel).toBe('Apples');
			expect(concept.rightLabel).toBe('sleep');
			expect(concept.similarity).toBe(0.92);
		});

		it('resolves the existing concept label from the Spatial View-Graph (the suggestion carries only an id)', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const concept = store.items.find((i) => i.kind === 'concept') as HousekeepingItem;
			expect(concept.rightLabel).toBe('sleep');
		});

		it('shows the borderline pair and a similarity score for a type suggestion, resolving the near-match label from GET /ontology', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const type = store.items.find((i) => i.kind === 'type') as HousekeepingItem;
			expect(type.leftLabel).toBe('Endangers');
			expect(type.rightLabel).toBe('Affects');
			expect(type.similarity).toBe(0.88);
		});

		it('carries NO inference evidence — only similarity scores (no evidence/path/snapshot field on items)', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			for (const item of store.items) {
				expect(item).not.toHaveProperty('evidencePath');
				expect(item).not.toHaveProperty('snapshot');
				expect(item).not.toHaveProperty('rationale');
			}
		});

		it('excludes type proposals with no near-match (pure new types are governance, not merge confirmations)', async () => {
			const pureNew: OntologyTypeProposal = {
				id: 34,
				slug: 'calms',
				label: 'Calms',
				description: 'Soothes.',
				merge_of: null,
				status: 'pending',
				near_match_slug: null,
				near_match_similarity: null
			};
			api = apiStub({ getOntologyProposals: vi.fn(async () => ({ proposals: [TYPE_PROPOSAL, pureNew] }) satisfies OntologyProposalsResponse) });
			const store = new HousekeepingStore(api);
			await store.load();
			expect(store.items.filter((i) => i.kind === 'type')).toHaveLength(1);
			expect(store.items.find((i) => i.id === 34)).toBeUndefined();
		});

		it('flips to error and does not populate the queue when a fetch rejects', async () => {
			api = apiStub({ getMergeSuggestions: vi.fn(async () => { throw new Error('GET /merge-suggestions failed: 401'); }) });
			const store = new HousekeepingStore(api);
			await store.load();
			expect(store.status).toBe('error');
			expect(store.error).toMatch(/401/);
			expect(store.items).toEqual([]);
		});
	});

	describe('confirmMerge — POST to the backend then optimistically merge the result into the Spatial View-Graph', () => {
		it('POSTs the concept-merge approve and folds the new concept into the survivor in the Spatial View-Graph', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const before = store.snapshot!;
			expect(before.concepts).toHaveLength(3);

			await store.confirmMerge(11, 'concept');

			expect(api.approveMergeSuggestion).toHaveBeenCalledWith(11);
			const after = store.snapshot!;
			expect(after.concepts.map((c) => c.id).sort()).toEqual(['1', '3']);
			expect(after.edges.find((e) => e.id === 'e2')?.target_concept_id).toBe('1');
		});

		it('removes the confirmed concept suggestion from the queue after merge', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			await store.confirmMerge(11, 'concept');
			expect(store.items.find((i) => i.id === 11 && i.kind === 'concept')).toBeUndefined();
		});

		it('POSTs the type-merge approve and optimistically retags edges of the merge_of type to the new slug', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			expect(store.snapshot!.edges.find((e) => e.id === 'e1')?.current_type).toBe('affects');

			await store.confirmMerge(33, 'type');

			expect(api.approveOntologyProposal).toHaveBeenCalledWith(33);
			expect(store.snapshot!.edges.find((e) => e.id === 'e1')?.current_type).toBe('endangers');
			expect(store.snapshot!.edges.find((e) => e.id === 'e2')?.current_type).toBe('disrupts');
		});

		it('adds the approved type to the local ontology context (read from GET /ontology)', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			await store.confirmMerge(33, 'type');
			expect(store.ontology?.edge_types.some((t) => t.slug === 'endangers')).toBe(true);
		});

		it('removes the confirmed type suggestion from the queue after merge', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			await store.confirmMerge(33, 'type');
			expect(store.items.find((i) => i.id === 33 && i.kind === 'type')).toBeUndefined();
		});

		it('does not optimistically merge when the POST fails (no local mutation on failure)', async () => {
			api = apiStub({ approveMergeSuggestion: vi.fn(async () => { throw new Error('POST /merge-suggestions/approve failed: 500'); }) });
			const store = new HousekeepingStore(api);
			await store.load();
			await expect(store.confirmMerge(11, 'concept')).rejects.toThrow(/500/);
			expect(store.snapshot!.concepts).toHaveLength(3);
			expect(store.items.find((i) => i.id === 11)).toBeDefined();
		});
	});

	describe('bifurcation — separate from the Endorsement Queue (ADR-0004)', () => {
		it('does not call any chat-inference / endorsement endpoint (that is the Endorsement Queue, #25)', async () => {
			const store = new HousekeepingStore(api);
			await store.load();
			const apiAny = api as unknown as Record<string, unknown>;
			expect(apiAny.getInferenceProposals).toBeUndefined();
			expect(apiAny.endorseInferenceProposal).toBeUndefined();
		});
	});
});
