import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
	createApiClient,
	type Invitation,
	type InvitationsResponse
} from '../../src/lib/api/client';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const INVITE: Invitation = {
	id: 7,
	token: 'Zb4Xc2vK9pR0sQ1tU3vW5xY7zA9bC1dE2fG3hI4jK5',
	created_by_user_id: '00000000-0000-0000-0000-000000000001',
	status: 'pending',
	created_at: 1_700_000_000,
	consumed_at: null,
	consumed_by_user_id: null,
	consumed_by_display_name: null
};

const INVITES_BODY: InvitationsResponse = {
	invitations: [
		INVITE,
		{
			id: 6,
			token: 'consumed-token-value',
			created_by_user_id: '00000000-0000-0000-0000-000000000001',
			status: 'consumed',
			created_at: 1_699_999_000,
			consumed_at: 1_700_000_500,
			consumed_by_user_id: '00000000-0000-0000-0000-000000000002',
			consumed_by_display_name: 'user_b'
		}
	]
};

describe('apiClient - admin invites surface against backend #73', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POST /admin/invites is credentialed so the auth cookie is sent', async () => {
		fetchMock.mockResolvedValue(okResponse(INVITE));
		const api = createApiClient({ fetch: fetchMock });
		await api.mintInvite();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/admin/invites');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
	});

	it('mintInvite() parses the invitation body (token + pending status)', async () => {
		fetchMock.mockResolvedValue(okResponse(INVITE));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.mintInvite();
		expect(res.id).toBe(7);
		expect(res.token).toBe(INVITE.token);
		expect(res.status).toBe('pending');
		expect(res.consumed_at).toBeNull();
		expect(res.consumed_by_user_id).toBeNull();
	});

	it('GET /admin/invites is credentialed', async () => {
		fetchMock.mockResolvedValue(okResponse(INVITES_BODY));
		const api = createApiClient({ fetch: fetchMock });
		await api.listInvites();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/admin/invites');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
	});

	it('listInvites() parses the { invitations } body with consumer info', async () => {
		fetchMock.mockResolvedValue(okResponse(INVITES_BODY));
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.listInvites();
		expect(res.invitations).toHaveLength(2);
		expect(res.invitations[0].status).toBe('pending');
		const consumed = res.invitations[1];
		expect(consumed.status).toBe('consumed');
		expect(consumed.consumed_by_display_name).toBe('user_b');
		expect(consumed.consumed_at).toBe(1_700_000_500);
	});

	it('mintInvite() throws on 403 (non-admin refused)', async () => {
		fetchMock.mockResolvedValue(new Response('forbidden', { status: 403 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.mintInvite()).rejects.toThrow(/403/);
	});

	it('listInvites() throws on 403 (non-admin refused)', async () => {
		fetchMock.mockResolvedValue(new Response('forbidden', { status: 403 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.listInvites()).rejects.toThrow(/403/);
	});

	it('mintInvite() throws on 401 (no session)', async () => {
		fetchMock.mockResolvedValue(
			new Response('unauthorized', { status: 401 })
		);
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.mintInvite()).rejects.toThrow(/401/);
	});
});
