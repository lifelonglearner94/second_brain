import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient, type Braindump } from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const BRAINDUMP: Braindump = {
	id: 42,
	verbatim: 'maria leaving tanks the timeline',
	cleaned: 'Maria leaving tanks the timeline.',
	created_at: 1_700_000_000
};

describe('apiClient.editBraindump — PATCH /braindumps/:id (backend #5, ADR-0007 error-correction)', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('PATCHes { verbatim } to /braindumps/:id credentialed so the session cookie reaches the authed edit path', async () => {
		fetchMock.mockResolvedValue(okResponse(BRAINDUMP));
		const api = createApiClient({ fetch: fetchMock });
		await api.editBraindump(42, 'maria leaving tanks the timeline');
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/braindumps/42');
		expect(init?.method).toBe('PATCH');
		expect(init?.credentials).toBe('include');
		expect(init?.headers).toMatchObject({ 'content-type': 'application/json' });
		expect(JSON.parse(init?.body as string)).toEqual({
			verbatim: 'maria leaving tanks the timeline'
		});
	});

	it('parses the returned Braindump — id and created_at stable, cleaned freshly re-derived by the backend', async () => {
		const edited: Braindump = {
			id: 42,
			verbatim: 'Maria is leaving, which tanks the timeline.',
			cleaned: 'Maria is leaving, which tanks the timeline.',
			created_at: 1_700_000_000
		};
		fetchMock.mockResolvedValue(okResponse(edited));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.editBraindump(
			42,
			'Maria is leaving, which tanks the timeline.'
		);
		expect(res).toEqual(edited);
		expect(res.id).toBe(42);
		expect(res.created_at).toBe(1_700_000_000);
		expect(res.cleaned).toBe('Maria is leaving, which tanks the timeline.');
	});

	it('throws on 400 (empty verbatim rejected by the backend) with the errorLabel prefix', async () => {
		fetchMock.mockResolvedValue(new Response('bad request', { status: 400 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.editBraindump(42, '')).rejects.toThrow(
			'PATCH /braindumps/:id failed: 400'
		);
	});

	it('throws on 404 so the Document Modal can keep the user in edit mode with an error', async () => {
		fetchMock.mockResolvedValue(new Response('not found', { status: 404 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.editBraindump(9999, 'x')).rejects.toThrow(
			'PATCH /braindumps/:id failed: 404'
		);
	});
});
