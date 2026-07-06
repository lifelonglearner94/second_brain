<script lang="ts">
	import { onMount } from 'svelte';
	import { adminInvites } from '$lib/state/admin-invites.svelte';

	onMount(() => {
		adminInvites.refresh();
	});

	async function onMint() {
		await adminInvites.mint();
	}

	async function onCopy(token: string) {
		await copyToClipboard(token, {
			onOk: () => adminInvites.markCopied(),
			onFail: () => adminInvites.clearCopied()
		});
	}

	// Issue #78: shared clipboard-write shape for the bare-token and invite-link
	// copy actions. On success flips a copied flag; on failure (clipboard
	// unavailable in an insecure context) clears it so the admin can copy
	// manually from the selectable token text.
	async function copyToClipboard(
		text: string,
		handlers: { onOk: () => void; onFail: () => void }
	): Promise<void> {
		try {
			await navigator.clipboard.writeText(text);
			handlers.onOk();
		} catch {
			handlers.onFail();
		}
	}

	// Issue #78: copy the full registration deep link (<origin>/login?invite=<token>)
	// so the admin can share a ready-to-click URL rather than just the bare token.
	// Independent copied-feedback from the bare-token copy action.
	async function onCopyLink(token: string) {
		await copyToClipboard(adminInvites.inviteLink(token), {
			onOk: () => adminInvites.markLinkCopied(),
			onFail: () => adminInvites.clearLinkCopied()
		});
	}

	// Per-row copy-link for the invitations list. Mirrors onCopyLink but flips
	// the row-local copied state so the "Copied" label shows on the row the
	// admin actually clicked, not on every pending row.
	async function onCopyRowLink(invite: {
		id: number;
		token: string;
	}): Promise<void> {
		await copyToClipboard(adminInvites.inviteLink(invite.token), {
			onOk: () => (copiedLinkRowId = invite.id),
			onFail: () => (copiedLinkRowId = null)
		});
	}

	function onDismissMinted() {
		adminInvites.clearLastMinted();
	}

	async function onRefresh() {
		await adminInvites.refresh();
	}

	function fmt(ts: number): string {
		try {
			return new Date(ts * 1000).toLocaleString();
		} catch {
			return String(ts);
		}
	}

	function shortToken(token: string): string {
		return token.length > 12 ? `${token.slice(0, 8)}…${token.slice(-4)}` : token;
	}

	// Issue #78: per-row "Copied" feedback for the copy-link action on pending
	// rows. The store's `linkCopied` serves the just-minted card (single
	// surface); the list has many rows, so we track which row's link was most
	// recently copied locally. Cleared when another row is copied.
	let copiedLinkRowId = $state<number | null>(null);
</script>

<main class="page invites-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="admin-invites-back">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				aria-hidden="true"
			>
				<path d="M15 18l-6-6 6-6" />
			</svg>
			Back to the app
		</a>
		<h1>Admin — invitations</h1>
		<p class="tagline" data-testid="admin-invites-summary">
			Mint single-use invitations to gate new passkey registrations.
			<code class="mono" data-testid="admin-invites-pending-count"
				>{adminInvites.pendingCount}</code
			>
			pending,
			<code class="mono" data-testid="admin-invites-consumed-count"
				>{adminInvites.consumedCount}</code
			>
			consumed.
		</p>
	</header>

	<section class="mint rise" aria-labelledby="mint-heading">
		<h2 id="mint-heading">Mint an invitation</h2>
		<p class="hint">
			The token is a one-time bearer — share it out-of-band with the invitee.
			It is shown once below; copy it now.
		</p>
		<button
			type="button"
			class="btn btn-primary"
			data-testid="admin-invites-mint"
			onclick={onMint}
			disabled={adminInvites.minting}
		>
			{adminInvites.minting ? 'Minting…' : 'Mint invite'}
		</button>
		{#if adminInvites.mintError}
			<p class="error pill pill-danger" data-testid="admin-invites-mint-error">
				{adminInvites.mintError}
			</p>
		{/if}
		{#if adminInvites.lastMinted}
			<div class="minted card" data-testid="admin-invites-minted">
				<div class="minted-head">
					<span class="badge badge-pending">pending</span>
					<span class="mono small">id #{adminInvites.lastMinted.id}</span>
				</div>
				<code
					class="token mono"
					data-testid="admin-invites-minted-token"
				>
					{adminInvites.lastMinted.token}
				</code>
				<div class="minted-actions">
					<button
						type="button"
						class="btn btn-secondary"
						data-testid="admin-invites-copy"
						onclick={() => onCopy(adminInvites.lastMinted!.token)}
					>
						{adminInvites.copied ? 'Copied' : 'Copy token'}
					</button>
					<button
						type="button"
						class="btn btn-secondary"
						data-testid="admin-invites-copy-link"
						data-invite-link={adminInvites.inviteLink(
							adminInvites.lastMinted!.token
						)}
						onclick={() => onCopyLink(adminInvites.lastMinted!.token)}
					>
						{adminInvites.linkCopied ? 'Copied' : 'Copy invite link'}
					</button>
					<button
						type="button"
						class="btn btn-ghost"
						data-testid="admin-invites-dismiss"
						onclick={onDismissMinted}
					>
						Done
					</button>
				</div>
			</div>
		{/if}
	</section>

	<section class="body rise" aria-labelledby="list-heading">
		<div class="list-head">
			<h2 id="list-heading">All invitations</h2>
			<button
				type="button"
				class="btn btn-secondary refresh"
				data-testid="admin-invites-refresh"
				onclick={onRefresh}
				disabled={adminInvites.status === 'loading'}
			>
				{adminInvites.status === 'loading' ? 'Refreshing…' : 'Refresh'}
			</button>
		</div>

		{#if adminInvites.status === 'loading' && adminInvites.invitations.length === 0}
			<div class="state card" data-testid="admin-invites-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Loading invitations…</span>
			</div>
		{:else if adminInvites.status === 'error'}
			<p class="state error pill pill-danger" data-testid="admin-invites-error">
				{adminInvites.error}
			</p>
		{:else if adminInvites.invitations.length === 0}
			<div class="state card empty" data-testid="admin-invites-empty">
				No invitations yet. Mint one above.
			</div>
		{:else}
			<ol class="invites" data-testid="admin-invites-list">
				{#each adminInvites.invitations as invite (invite.id)}
					<li
						class="invite card"
						data-testid="admin-invite-row"
						data-status={invite.status}
					>
						<div class="invite-head">
							<span
								class="badge"
								class:badge-pending={invite.status === 'pending'}
								class:badge-consumed={invite.status === 'consumed'}
								data-status={invite.status}
							>
								{invite.status}
							</span>
							<time class="ts mono">{fmt(invite.created_at)}</time>
							<span class="mono small">id #{invite.id}</span>
						</div>
						<code class="token mono" data-testid="admin-invite-token">
							{shortToken(invite.token)}
						</code>
						<div class="invite-meta">
							<span class="meta-row">
								<span class="meta-label">Created by</span>
								<code class="mono small">{invite.created_by_user_id}</code>
							</span>
							{#if invite.status === 'consumed'}
								<span class="meta-row">
									<span class="meta-label">Consumed by</span>
									<code class="mono small"
										>{invite.consumed_by_display_name ??
										invite.consumed_by_user_id ??
										'unknown'}</code
									>
								</span>
								{#if invite.consumed_at}
									<span class="meta-row">
										<span class="meta-label">Consumed at</span>
										<time class="mono small">{fmt(invite.consumed_at!)}</time>
									</span>
								{/if}
							{/if}
						</div>
					{#if invite.status === 'pending'}
						<div class="invite-actions">
							<button
								type="button"
								class="btn btn-secondary btn-sm"
								data-testid="admin-invite-copy-link"
								data-invite-link={adminInvites.inviteLink(invite.token)}
								onclick={() => onCopyRowLink(invite)}
							>
								{copiedLinkRowId === invite.id
									? 'Copied'
									: 'Copy invite link'}
							</button>
						</div>
					{/if}
				</li>
				{/each}
			</ol>
		{/if}
	</section>
</main>

<style>
	.invites-page {
		max-inline-size: 50rem;
		display: grid;
		gap: var(--space-5);
	}
	.page-head {
		display: grid;
		gap: var(--space-2);
		padding-block-end: var(--space-4);
		border-block-end: 1px solid var(--border-hairline);
	}
	.back-link {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		color: var(--fg-muted);
		font-size: var(--fs-13);
	}
	.back-link:hover {
		color: var(--accent);
	}
	.back-link svg {
		inline-size: 1rem;
		block-size: 1rem;
	}
	.page-head h1 {
		font-size: var(--fs-28);
		font-weight: 700;
	}
	.tagline {
		color: var(--fg-muted);
		font-size: var(--fs-14);
	}
	.tagline code {
		color: var(--accent);
		font-size: var(--fs-14);
		font-weight: 600;
	}

	.mint {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-5);
	}
	.mint h2 {
		font-size: var(--fs-18);
		font-weight: 700;
	}
	.hint {
		color: var(--fg-muted);
		font-size: var(--fs-13);
		margin: 0;
	}
	.mint .btn-primary {
		justify-self: start;
		min-block-size: 44px;
	}
	.error {
		padding: 0.6rem 0.85rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
	}

	.minted {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4);
		border-color: var(--border-accent);
	}
	.minted-head {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2) var(--space-3);
		align-items: center;
		font-size: var(--fs-12);
	}
	.token {
		display: block;
		padding: var(--space-3) var(--space-4);
		font-size: var(--fs-13);
		line-height: 1.5;
		color: var(--fg);
		background: var(--bg-sunken);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		word-break: break-all;
		overflow-wrap: anywhere;
		user-select: all;
	}
	.minted-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.invite-actions {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
		justify-content: flex-end;
	}
	.btn-sm {
		min-block-size: 36px;
		font-size: var(--fs-13);
		padding: 0 var(--space-3);
	}

	.list-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.list-head h2 {
		font-size: var(--fs-18);
		font-weight: 700;
	}
	.refresh {
		min-block-size: 40px;
	}

	.state {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-5);
		color: var(--fg-muted);
		font-size: var(--fs-14);
	}
	.state.empty {
		justify-content: center;
	}
	.dot-pulse {
		inline-size: 7px;
		block-size: 7px;
		border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 0 0 var(--accent-glow);
		animation: pulse 1.6s var(--ease) infinite;
	}
	@keyframes pulse {
		0% {
			box-shadow: 0 0 0 0 var(--accent-glow);
		}
		70% {
			box-shadow: 0 0 0 8px transparent;
		}
		100% {
			box-shadow: 0 0 0 0 transparent;
		}
	}

	.invites {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-2);
	}
	.invite {
		display: grid;
		gap: var(--space-2);
		padding: var(--space-3) var(--space-4);
		transition: border-color var(--dur-1) var(--ease);
	}
	.invite:hover {
		border-color: var(--border-strong);
	}
	.invite-head {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2) var(--space-3);
		align-items: center;
		font-size: var(--fs-12);
	}
	.badge {
		padding: 0.1rem 0.5rem;
		font-family: var(--font-mono);
		font-size: var(--fs-12);
		font-weight: 600;
		letter-spacing: 0.04em;
		text-transform: uppercase;
		border-radius: var(--radius-sm);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		color: var(--fg-muted);
	}
	.badge-pending {
		color: var(--accent);
		background: var(--accent-soft);
		border-color: var(--border-accent);
	}
	.badge-consumed {
		color: var(--fg-subtle);
		background: var(--bg-sunken);
	}
	.ts {
		color: var(--fg-subtle);
	}
	.small {
		font-size: var(--fs-12);
	}
	.invite .token {
		font-size: var(--fs-12);
	}
	.invite-meta {
		display: grid;
		gap: var(--space-1);
		font-size: var(--fs-12);
		color: var(--fg-muted);
	}
	.meta-row {
		display: flex;
		gap: var(--space-2);
		align-items: baseline;
		flex-wrap: wrap;
	}
	.meta-label {
		color: var(--fg-subtle);
		min-inline-size: 6rem;
	}

	.btn-ghost {
		background: transparent;
		border: 1px solid transparent;
		color: var(--fg-muted);
		cursor: pointer;
		min-block-size: 40px;
		padding: 0 var(--space-3);
	}
	.btn-ghost:hover {
		color: var(--fg);
	}
</style>
