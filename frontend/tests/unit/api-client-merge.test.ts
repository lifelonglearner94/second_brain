import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
	createApiClient,
	type ConceptMergeSuggestion,
	type Ontology,
	type OntologyTypeProposal,
	type OntologyProposalsResponse
} from '../../src/lib/api/client';

function okResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'content-type': 'application/json' }
	});
}

function noContent(): Response {
	return new Response(null, { status: 204 });
}

describe('apiClient.getMergeSuggestions - backend #7 GET /merge-suggestions (concept pairs)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	const SUGGESTIONS: ConceptMergeSuggestion[] = [
		{
			id: 1,
			kind: 'concept',
			braindump_id: 5,
			new_concept_label: 'Apples',
			new_concept_id: 42,
			existing_concept_id: 7,
			similarity: 0.92,
			status: 'pending',
			created_at: 1_700_000_000
		}
	];

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /merge-suggestions and parses the concept merge-suggestion array', async () => {
		fetchMock.mockResolvedValue(okResponse(SUGGESTIONS));
		const api = createApiClient({ fetch: fetchMock });
		const suggestions = await api.getMergeSuggestions();
		expect(suggestions).toEqual(SUGGESTIONS);
		expect(suggestions[0]?.similarity).toBe(0.92);
		expect(suggestions[0]?.new_concept_id).toBe(42);
		expect(suggestions[0]?.existing_concept_id).toBe(7);
	});

	it('hits the /merge-suggestions path under the configured base URL with credentials', async () => {
		fetchMock.mockResolvedValue(okResponse(SUGGESTIONS));
		const api = createApiClient({
			baseUrl: 'https://brain.example.test',
			fetch: fetchMock
		});
		await api.getMergeSuggestions();
		expect(fetchMock.mock.calls[0][0]).toBe(
			'https://brain.example.test/merge-suggestions'
		);
		expect(fetchMock.mock.calls[0][1]).toMatchObject({
			credentials: 'include'
		});
	});

	it('throws on a non-2xx so the Housekeeping Queue can surface a load error', async () => {
		fetchMock.mockResolvedValue(new Response('nope', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getMergeSuggestions()).rejects.toThrow(/401/);
	});
});

describe('apiClient.approveMergeSuggestion - backend #7 POST /merge-suggestions/{id}/approve', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs the approve and resolves on 204 No Content (backend folds new into existing)', async () => {
		fetchMock.mockResolvedValue(noContent());
		const api = createApiClient({ fetch: fetchMock });
		await api.approveMergeSuggestion(1);
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/merge-suggestions/1/approve');
		expect(init?.method).toBe('POST');
		expect(init).toMatchObject({ credentials: 'include' });
	});

	it('does not require a JSON body (the backend returns 204 with no content)', async () => {
		fetchMock.mockResolvedValue(noContent());
		const api = createApiClient({ fetch: fetchMock });
		await api.approveMergeSuggestion(7);
		expect(fetchMock.mock.calls[0][1]?.body).toBeNull();
	});

	it('throws on a non-2xx so the optimistic merge is not applied on failure', async () => {
		fetchMock.mockResolvedValue(new Response('gone', { status: 404 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.approveMergeSuggestion(99)).rejects.toThrow(/404/);
	});
});

describe('apiClient.rejectMergeSuggestion - backend #7 POST /merge-suggestions/{id}/reject', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs the reject and resolves on 204 No Content', async () => {
		fetchMock.mockResolvedValue(noContent());
		const api = createApiClient({ fetch: fetchMock });
		await api.rejectMergeSuggestion(2);
		expect(fetchMock.mock.calls[0][0]).toBe('/api/merge-suggestions/2/reject');
		expect(fetchMock.mock.calls[0][1]?.method).toBe('POST');
	});
});

describe('apiClient.getOntology - backend #3 GET /ontology (type context, public)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	const ONTOLOGY: Ontology = {
		edge_types: [
			{ slug: 'affects', label: 'Affects', description: 'Has an effect on.' },
			{ slug: 'disrupts', label: 'Disrupts', description: 'Interferes with.' }
		]
	};

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /ontology and parses the governed edge-type vocabulary', async () => {
		fetchMock.mockResolvedValue(okResponse(ONTOLOGY));
		const api = createApiClient({ fetch: fetchMock });
		const ontology = await api.getOntology();
		expect(ontology).toEqual(ONTOLOGY);
		expect(ontology.edge_types[0]?.slug).toBe('affects');
	});

	it('hits the /ontology path under the configured base URL with credentials', async () => {
		fetchMock.mockResolvedValue(okResponse(ONTOLOGY));
		const api = createApiClient({
			baseUrl: 'https://brain.example.test',
			fetch: fetchMock
		});
		await api.getOntology();
		expect(fetchMock.mock.calls[0][0]).toBe(
			'https://brain.example.test/ontology'
		);
		expect(fetchMock.mock.calls[0][1]).toMatchObject({
			credentials: 'include'
		});
	});
});

describe('apiClient.getOntologyProposals - backend #3 GET /ontology/proposals (type-merge queue)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	const PROPOSAL: OntologyTypeProposal = {
		id: 3,
		slug: 'endangers',
		label: 'Endangers',
		description: 'Causes harm to.',
		merge_of: 'affects',
		status: 'pending',
		near_match_slug: 'affects',
		near_match_similarity: 0.88
	};

	const RESPONSE: OntologyProposalsResponse = { proposals: [PROPOSAL] };

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /ontology/proposals and parses the proposals wrapper', async () => {
		fetchMock.mockResolvedValue(okResponse(RESPONSE));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.getOntologyProposals();
		expect(res).toEqual(RESPONSE);
		expect(res.proposals[0]?.near_match_similarity).toBe(0.88);
		expect(res.proposals[0]?.merge_of).toBe('affects');
	});

	it('hits the /ontology/proposals path under the configured base URL with credentials', async () => {
		fetchMock.mockResolvedValue(okResponse(RESPONSE));
		const api = createApiClient({
			baseUrl: 'https://brain.example.test',
			fetch: fetchMock
		});
		await api.getOntologyProposals();
		expect(fetchMock.mock.calls[0][0]).toBe(
			'https://brain.example.test/ontology/proposals'
		);
		expect(fetchMock.mock.calls[0][1]).toMatchObject({
			credentials: 'include'
		});
	});
});

describe('apiClient.approveOntologyProposal - backend #3 POST /ontology/proposals/{id}/approve', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs the approve and returns the approved proposal (status flips to approved)', async () => {
		const approved: OntologyTypeProposal = {
			id: 3,
			slug: 'endangers',
			label: 'Endangers',
			description: 'Causes harm to.',
			merge_of: 'affects',
			status: 'approved',
			near_match_slug: 'affects',
			near_match_similarity: 0.88
		};
		fetchMock.mockResolvedValue(okResponse(approved));
		const api = createApiClient({ fetch: fetchMock });
		const result = await api.approveOntologyProposal(3);
		expect(fetchMock.mock.calls[0][0]).toBe(
			'/api/ontology/proposals/3/approve'
		);
		expect(fetchMock.mock.calls[0][1]?.method).toBe('POST');
		expect(fetchMock.mock.calls[0][1]).toMatchObject({
			credentials: 'include'
		});
		expect(result.status).toBe('approved');
		expect(result.slug).toBe('endangers');
	});

	it('throws on a non-2xx (e.g. 409 conflict when already resolved)', async () => {
		fetchMock.mockResolvedValue(new Response('conflict', { status: 409 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.approveOntologyProposal(3)).rejects.toThrow(/409/);
	});
});
