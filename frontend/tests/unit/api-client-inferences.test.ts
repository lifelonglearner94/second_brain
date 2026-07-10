import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
	createApiClient,
	type ChatInferenceProposal,
	type EvidenceEdge,
	type ThematicSnapshot
} from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const STRUCTURAL: ChatInferenceProposal = {
	id: 101,
	mode: 'structural_inference',
	source_concept_id: 1,
	target_concept_id: 3,
	proposed_type: 'endangers',
	evidence_path: [
		{ source_concept_id: 1, edge_type: 'endangers', target_concept_id: 2 },
		{ source_concept_id: 2, edge_type: 'depends_on', target_concept_id: 3 }
	],
	rationale: 'Maria endangers the launch the beta depends on',
	status: 'pending',
	created_at: 1_700_000_000,
	resolved_at: null,
	snapshot: null
};

const THEMATIC: ChatInferenceProposal = {
	id: 102,
	mode: 'thematic_inference',
	source_concept_id: 10,
	target_concept_id: 12,
	proposed_type: 'correlates_with',
	evidence_path: [],
	rationale: 'Cluster density suggests a bridge',
	status: 'pending',
	created_at: 1_700_000_010,
	resolved_at: null,
	snapshot: {
		id: 55,
		braindump_ids: [201, 202, 203],
		concept_ids: [10, 11, 12],
		captured_at: 1_700_000_010
	}
};

describe('apiClient - chat-inference proposal queue (backend #11/#13, ADR-0006)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GET /chat/inferences is credentialed so the auth cookie reaches the endpoint', async () => {
		fetchMock.mockResolvedValue(okResponse([]));
		const api = createApiClient({ fetch: fetchMock });
		await api.getInferenceProposals();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/chat/inferences');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
	});

	it('parses both structural and thematic proposals from the queue', async () => {
		fetchMock.mockResolvedValue(okResponse([THEMATIC, STRUCTURAL]));
		const api = createApiClient({ fetch: fetchMock });
		const proposals = await api.getInferenceProposals();
		expect(proposals).toHaveLength(2);
		expect(proposals[0]).toEqual(THEMATIC);
		expect(proposals[1]).toEqual(STRUCTURAL);
	});

	it('preserves the traversable evidence_path hops for a structural proposal', async () => {
		fetchMock.mockResolvedValue(okResponse([STRUCTURAL]));
		const api = createApiClient({ fetch: fetchMock });
		const [proposal] = await api.getInferenceProposals();
		expect(proposal.evidence_path).toEqual<EvidenceEdge[]>([
			{ source_concept_id: 1, edge_type: 'endangers', target_concept_id: 2 },
			{ source_concept_id: 2, edge_type: 'depends_on', target_concept_id: 3 }
		]);
		expect(proposal.snapshot).toBeNull();
	});

	it('preserves the frozen Thematic Snapshot for a thematic proposal', async () => {
		fetchMock.mockResolvedValue(okResponse([THEMATIC]));
		const api = createApiClient({ fetch: fetchMock });
		const [proposal] = await api.getInferenceProposals();
		expect(proposal.evidence_path).toEqual([]);
		expect(proposal.snapshot).toEqual<ThematicSnapshot>({
			id: 55,
			braindump_ids: [201, 202, 203],
			concept_ids: [10, 11, 12],
			captured_at: 1_700_000_010
		});
	});

	it('throws on a non-2xx response (401 when no session)', async () => {
		fetchMock.mockResolvedValue(new Response('unauthorized', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getInferenceProposals()).rejects.toThrow(/401/);
	});
});

describe('apiClient.endorseInferenceProposal - POST /chat/inferences/{id}/endorse', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	const ENDORSED: ChatInferenceProposal = {
		...STRUCTURAL,
		status: 'endorsed',
		resolved_at: 1_700_000_030
	};

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs to /chat/inferences/{id}/endorse with credentials', async () => {
		fetchMock.mockResolvedValue(okResponse(ENDORSED));
		const api = createApiClient({ fetch: fetchMock });
		await api.endorseInferenceProposal(101);
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/chat/inferences/101/endorse');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
	});

	it('parses the endorsed proposal returned by the backend', async () => {
		fetchMock.mockResolvedValue(okResponse(ENDORSED));
		const api = createApiClient({ fetch: fetchMock });
		const endorsed = await api.endorseInferenceProposal(101);
		expect(endorsed.id).toBe(101);
		expect(endorsed.status).toBe('endorsed');
		expect(endorsed.resolved_at).toBe(1_700_000_030);
		expect(endorsed.source_concept_id).toBe(1);
		expect(endorsed.target_concept_id).toBe(3);
		expect(endorsed.proposed_type).toBe('endangers');
	});

	it('throws on a non-2xx response (409 when already resolved)', async () => {
		fetchMock.mockResolvedValue(new Response('conflict', { status: 409 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.endorseInferenceProposal(101)).rejects.toThrow(/409/);
	});
});
