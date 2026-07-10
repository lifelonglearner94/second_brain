import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
	createApiClient,
	type ChatResponse,
	type Braindump,
	type ChatCitation
} from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const GROUNDED: ChatResponse = {
	answer:
		'Q3 launch is at risk because Maria is leaving [bd:42] [edge:Maria -endangers→ Q3 launch].',
	citations: [
		{
			id: 42,
			verbatim: 'maria leaving tanks the timeline',
			cleaned: 'Maria leaving tanks the timeline.',
			created_at: 1_700_000_000,
			score: 1.0,
			source: 'subgraph'
		}
	],
	paths: [
		{
			source_concept_id: 7,
			source_concept_label: 'Maria',
			target_concept_id: 11,
			target_concept_label: 'Q3 launch',
			edge_type: 'endangers'
		}
	],
	silent: false,
	mode: 'seed_then_expand'
};

const SILENT: ChatResponse = {
	answer: 'you haven\u2019t told me about that',
	citations: [],
	paths: [],
	silent: true,
	mode: 'no_seed_fallback'
};

const BRAINDUMP: Braindump = {
	id: 42,
	verbatim: 'maria leaving tanks the timeline',
	cleaned: 'Maria leaving tanks the timeline.',
	created_at: 1_700_000_000
};

describe('apiClient - chat surface against backend #10 (POST /chat, ADR-0005)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POSTs /chat with the query as JSON, credentialed', async () => {
		fetchMock.mockResolvedValue(okResponse(GROUNDED));
		const api = createApiClient({ fetch: fetchMock });
		await api.chat('is Q3 at risk?');
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/chat');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
		expect(init?.headers).toMatchObject({ 'content-type': 'application/json' });
		expect(JSON.parse(init?.body as string)).toEqual({
			query: 'is Q3 at risk?'
		});
	});

	it('parses the grounded answer, citations, traversed paths, and retrieval mode', async () => {
		fetchMock.mockResolvedValue(okResponse(GROUNDED));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.chat('is Q3 at risk?');
		expect(res.silent).toBe(false);
		expect(res.answer).toBe(GROUNDED.answer);
		expect(res.citations).toHaveLength(1);
		const cite = res.citations[0] as ChatCitation;
		expect(cite.id).toBe(42);
		expect(cite.cleaned).toBe('Maria leaving tanks the timeline.');
		expect(cite.source).toBe('subgraph');
		expect(res.paths[0]?.edge_type).toBe('endangers');
		expect(res.mode).toBe('seed_then_expand');
	});

	it('parses the Explicit Silence response: silent=true, no citations, no paths', async () => {
		fetchMock.mockResolvedValue(okResponse(SILENT));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.chat('what is the meaning of life?');
		expect(res.silent).toBe(true);
		expect(res.citations).toEqual([]);
		expect(res.paths).toEqual([]);
		expect(res.answer).toBe('you haven\u2019t told me about that');
		expect(res.mode).toBe('no_seed_fallback');
	});

	it('throws on a non-2xx response (401 when no session)', async () => {
		fetchMock.mockResolvedValue(new Response('unauthorized', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.chat('anything')).rejects.toThrow(/401/);
	});
});

describe('apiClient - braindump reader against backend #5 (GET /braindumps/:id)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('GETs /braindumps/:id credentialed and parses both renderings', async () => {
		fetchMock.mockResolvedValue(okResponse(BRAINDUMP));
		const api = createApiClient({ fetch: fetchMock });
		const bd = await api.getBraindump(42);
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/braindumps/42');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
		expect(bd).toEqual(BRAINDUMP);
		expect(bd.cleaned).toBe('Maria leaving tanks the timeline.');
		expect(bd.verbatim).toBe('maria leaving tanks the timeline');
	});

	it('throws on 404 so the Document Modal can render its not-found state', async () => {
		fetchMock.mockResolvedValue(new Response('not found', { status: 404 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getBraindump(9999)).rejects.toThrow(/404/);
	});
});
