import { describe, it, expect, vi } from 'vitest';
import {
	AdminInviteStore,
	type AdminInviteApi
} from '../../src/lib/state/admin-invites.svelte';
import type { Invitation, InvitationsResponse } from '../../src/lib/api/client';

const PENDING: Invitation = {
	id: 7,
	token: 'pending-token-abc',
	created_by_user_id: '00000000-0000-0000-0000-000000000001',
	status: 'pending',
	created_at: 1_700_000_000,
	consumed_at: null,
	consumed_by_user_id: null,
	consumed_by_display_name: null
};

const CONSUMED: Invitation = {
	id: 6,
	token: 'consumed-token-xyz',
	created_by_user_id: '00000000-0000-0000-0000-000000000001',
	status: 'consumed',
	created_at: 1_699_999_000,
	consumed_at: 1_700_000_500,
	consumed_by_user_id: '00000000-0000-0000-0000-000000000002',
	consumed_by_display_name: 'user_b'
};

const LIST: InvitationsResponse = { invitations: [CONSUMED] };

function apiStub(api: Partial<AdminInviteApi>): AdminInviteApi {
	return {
		mintInvite: api.mintInvite ?? vi.fn<AdminInviteApi['mintInvite']>(),
		listInvites: api.listInvites ?? vi.fn<AdminInviteApi['listInvites']>()
	};
}

describe('AdminInviteStore - admin invite mint+list over backend #73', () => {
	it('starts idle with no invitations and no minted token', () => {
		const store = new AdminInviteStore(
			apiStub({
				mintInvite: vi
					.fn<AdminInviteApi['mintInvite']>()
					.mockRejectedValue(new Error('no')),
				listInvites: vi
					.fn<AdminInviteApi['listInvites']>()
					.mockRejectedValue(new Error('no'))
			})
		);
		expect(store.status).toBe('idle');
		expect(store.invitations).toEqual([]);
		expect(store.lastMinted).toBeNull();
		expect(store.minting).toBe(false);
		expect(store.copied).toBe(false);
	});

	it('refresh() loads invitations from the backend and flips to loaded', async () => {
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockResolvedValue(LIST);
		const store = new AdminInviteStore(apiStub({ listInvites }));
		await store.refresh();
		expect(listInvites).toHaveBeenCalledOnce();
		expect(store.status).toBe('loaded');
		expect(store.invitations).toEqual(LIST.invitations);
	});

	it('refresh() surfaces the error message on failure (e.g. 403)', async () => {
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockRejectedValue(new Error('GET /admin/invites failed: 403'));
		const store = new AdminInviteStore(apiStub({ listInvites }));
		await store.refresh();
		expect(store.status).toBe('error');
		expect(store.error).toMatch(/403/);
		expect(store.invitations).toEqual([]);
	});

	it('mint() calls the API, sets lastMinted (token shown once), and prepends to the list', async () => {
		const mintInvite = vi
			.fn<AdminInviteApi['mintInvite']>()
			.mockResolvedValue(PENDING);
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockResolvedValue(LIST);
		const store = new AdminInviteStore(apiStub({ mintInvite, listInvites }));
		await store.refresh();
		await store.mint();
		expect(mintInvite).toHaveBeenCalledOnce();
		expect(store.lastMinted).toEqual(PENDING);
		expect(store.mintError).toBeNull();
		// The freshly minted invite appears at the top of the list.
		expect(store.invitations[0]).toEqual(PENDING);
		expect(store.invitations).toHaveLength(2);
	});

	it('mint() flips to mintError and does not clobber the list on failure (403)', async () => {
		const mintInvite = vi
			.fn<AdminInviteApi['mintInvite']>()
			.mockRejectedValue(new Error('POST /admin/invites failed: 403'));
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockResolvedValue(LIST);
		const store = new AdminInviteStore(apiStub({ mintInvite, listInvites }));
		await store.refresh();
		await store.mint();
		expect(store.mintError).toMatch(/403/);
		expect(store.lastMinted).toBeNull();
		expect(store.invitations).toEqual(LIST.invitations);
	});

	it('mint() is mutually exclusive - minting is true while in flight, false after', async () => {
		let resolveMint: (v: Invitation) => void = () => {};
		const mintInvite = vi
			.fn<AdminInviteApi['mintInvite']>()
			.mockImplementation(
				() => new Promise<Invitation>((r) => (resolveMint = r))
			);
		const store = new AdminInviteStore(apiStub({ mintInvite }));
		const pending = store.mint();
		expect(store.minting).toBe(true);
		resolveMint(PENDING);
		await pending;
		expect(store.minting).toBe(false);
	});

	it('clearLastMinted() drops the once-shown token so it leaves memory', async () => {
		const mintInvite = vi
			.fn<AdminInviteApi['mintInvite']>()
			.mockResolvedValue(PENDING);
		const store = new AdminInviteStore(apiStub({ mintInvite }));
		await store.mint();
		expect(store.lastMinted).not.toBeNull();
		store.clearLastMinted();
		expect(store.lastMinted).toBeNull();
	});

	it('markCopied() toggles the copied flag for copy-button feedback', async () => {
		const mintInvite = vi
			.fn<AdminInviteApi['mintInvite']>()
			.mockResolvedValue(PENDING);
		const store = new AdminInviteStore(apiStub({ mintInvite }));
		await store.mint();
		expect(store.copied).toBe(false);
		store.markCopied();
		expect(store.copied).toBe(true);
		store.clearCopied();
		expect(store.copied).toBe(false);
	});

	describe('issue #78 - copy-invite-link affordance', () => {
		it('inviteLink(token) builds <origin>/login?invite=<token> from the live browser origin', () => {
			const store = new AdminInviteStore(apiStub({}));
			expect(store.inviteLink('tok-abc')).toBe(
				`${window.location.origin}/login?invite=tok-abc`
			);
		});

		it('inviteLink(token) encodeURIComponent-encodes the token so reserved query characters stay well-formed', () => {
			const store = new AdminInviteStore(apiStub({}));
			// base64url tokens (`-`/`_`) are URL-safe and pass through unchanged;
			// a token containing `&`/`=`/`+` (if the charset ever changed) would
			// otherwise corrupt the deep link.
			expect(store.inviteLink('a&b=c+')).toBe(
				`${window.location.origin}/login?invite=${encodeURIComponent('a&b=c+')}`
			);
		});

		it('markLinkCopied() toggles linkCopied independently of copied (clicking one does not flip the other)', async () => {
			const mintInvite = vi
				.fn<AdminInviteApi['mintInvite']>()
				.mockResolvedValue(PENDING);
			const store = new AdminInviteStore(apiStub({ mintInvite }));
			await store.mint();
			expect(store.copied).toBe(false);
			expect(store.linkCopied).toBe(false);

			store.markCopied();
			expect(store.copied).toBe(true);
			expect(store.linkCopied).toBe(false);

			store.markLinkCopied();
			expect(store.linkCopied).toBe(true);
			expect(store.copied).toBe(true);

			store.clearLinkCopied();
			expect(store.linkCopied).toBe(false);
			expect(store.copied).toBe(true);
		});

		it('mint() and clearLastMinted() reset linkCopied alongside copied', async () => {
			const mintInvite = vi
				.fn<AdminInviteApi['mintInvite']>()
				.mockResolvedValue(PENDING);
			const store = new AdminInviteStore(apiStub({ mintInvite }));
			await store.mint();
			store.markCopied();
			store.markLinkCopied();

			await store.mint();
			expect(store.copied).toBe(false);
			expect(store.linkCopied).toBe(false);

			store.markLinkCopied();
			store.clearLastMinted();
			expect(store.linkCopied).toBe(false);
		});
	});

	it('pendingCount is the number of pending invitations only', async () => {
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockResolvedValue({ invitations: [PENDING, CONSUMED, PENDING] });
		const store = new AdminInviteStore(apiStub({ listInvites }));
		await store.refresh();
		expect(store.pendingCount).toBe(2);
	});

	it('consumedCount is the number of consumed invitations only', async () => {
		const listInvites = vi
			.fn<AdminInviteApi['listInvites']>()
			.mockResolvedValue({ invitations: [PENDING, CONSUMED, CONSUMED] });
		const store = new AdminInviteStore(apiStub({ listInvites }));
		await store.refresh();
		expect(store.consumedCount).toBe(2);
	});
});
