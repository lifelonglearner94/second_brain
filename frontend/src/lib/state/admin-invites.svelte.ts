import { apiClient } from '$lib/api';
import type { Invitation, InvitationsResponse } from '$lib/api/client';

export type AdminInviteStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type AdminInviteApi = {
	mintInvite(): Promise<Invitation>;
	listInvites(): Promise<InvitationsResponse>;
};

function browserOrigin(): string {
	return typeof window !== 'undefined' ? window.location.origin : '';
}

/**
 * Admin tab state for the invitation minter (backend #73). The store mints a
 * single-use invite and lists all outstanding/consumed invitations. The most
 * recently minted token is held in `lastMinted` so the page can show it once
 * (copyable) and then drop it from memory via `clearLastMinted` - the token is
 * a bearer secret, so it should not linger in reactive state longer than the
 * admin needs to copy it. The canonical record of every invite lives on the
 * backend and is re-fetched via `refresh`.
 */
export class AdminInviteStore {
	status = $state<AdminInviteStatus>('idle');
	invitations = $state<Invitation[]>([]);
	error = $state<string | null>(null);

	minting = $state(false);
	mintError = $state<string | null>(null);
	lastMinted = $state<Invitation | null>(null);
	copied = $state(false);
	// Issue #78: independent feedback for the "Copy invite link" affordance so
	// copy-token and copy-link can each show their own "Copied" state without
	// flipping the other's label.
	linkCopied = $state(false);

	constructor(private api: AdminInviteApi) {}

	async refresh(): Promise<void> {
		this.status = 'loading';
		this.error = null;
		try {
			const res = await this.api.listInvites();
			this.invitations = res.invitations;
			this.status = 'loaded';
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
		}
	}

	async mint(): Promise<void> {
		this.minting = true;
		this.mintError = null;
		this.copied = false;
		this.linkCopied = false;
		try {
			const invite = await this.api.mintInvite();
			this.lastMinted = invite;
			// Prepend so the freshest mint is at the top, mirroring the
			// backend's newest-first ordering.
			this.invitations = [invite, ...this.invitations];
		} catch (e) {
			this.mintError = e instanceof Error ? e.message : String(e);
		} finally {
			this.minting = false;
		}
	}

	clearLastMinted(): void {
		this.lastMinted = null;
		this.copied = false;
		this.linkCopied = false;
	}

	markCopied(): void {
		this.copied = true;
	}

	clearCopied(): void {
		this.copied = false;
	}

	markLinkCopied(): void {
		this.linkCopied = true;
	}

	clearLinkCopied(): void {
		this.linkCopied = false;
	}

	/**
	 * Issue #78: build the full registration deep link for an invitation token
	 * - `<origin>/login?invite=<token>` - so an admin can share a ready-to-
	 * click URL out-of-band. The PWA is static, so the deployed origin is read
	 * client-side from `window.location.origin` (correct in dev, preview, and
	 * production). The token is `encodeURIComponent`-encoded so the deep link
	 * stays well-formed even if the backend token charset ever includes
	 * reserved query characters; the login page decodes it back via
	 * `searchParams.get('invite')`.
	 */
	inviteLink(token: string): string {
		return `${browserOrigin()}/login?invite=${encodeURIComponent(token)}`;
	}

	pendingCount = $derived(
		this.invitations.filter((i) => i.status === 'pending').length
	);

	consumedCount = $derived(
		this.invitations.filter((i) => i.status === 'consumed').length
	);
}

export const adminInvites = new AdminInviteStore(apiClient);
