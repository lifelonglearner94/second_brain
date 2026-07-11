<script lang="ts">
	import { goto } from '$app/navigation';
	import { apiClient } from '$lib/api';
	import { session } from '$lib/state/session.svelte';
	import { createIngestApi, type IngestResponse } from '$lib/capture/ingest';
	import { pendingCaptures } from '$lib/state/pending-captures.svelte';
	import { graphStore } from '$lib/state/graph.svelte';
	import ActiveCapture from '$lib/capture/ActiveCapture.svelte';
	import { onMount } from 'svelte';

	let busy = $state(false);
	let logoutError = $state<string | null>(null);
	let headerTaps = $state(0);
	let online = $state(
		typeof navigator !== 'undefined' ? navigator.onLine : true
	);

	const ingestApi = createIngestApi(apiClient, () => graphStore.cursor);

	function onHeaderTap() {
		headerTaps += 1;
	}

	function onIngest(res: IngestResponse): void {
		graphStore.mergeIngest(res);
	}

	async function onLogout() {
		busy = true;
		logoutError = null;
		try {
			await apiClient.logout();
			session.clear();
			await goto('/login', { replaceState: true });
		} catch (e) {
			logoutError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	onMount(() => {
		function handleConnectivity(): void {
			online = typeof navigator !== 'undefined' ? navigator.onLine : true;
			if (online) {
				void pendingCaptures.load();
			}
		}
		globalThis.addEventListener('online', handleConnectivity);
		globalThis.addEventListener('offline', handleConnectivity);
		void pendingCaptures.load();
		return () => {
			globalThis.removeEventListener('online', handleConnectivity);
			globalThis.removeEventListener('offline', handleConnectivity);
		};
	});
</script>

<main class="page app-home">
	<header class="appbar rise">
		<div class="appbar-brand">
			<button
				type="button"
				data-testid="app-title"
				class="wordmark"
				onclick={onHeaderTap}
				aria-label="Second Brain"
			>
				Second Brain
			</button>
			<span class="identity">
				<span class="identity-dot" aria-hidden="true"></span>
				<span class="identity-label">Signed in as</span>
				<code class="mono" data-testid="user-id">{session.userId}</code>
			</span>
		</div>

		<div class="appbar-actions">
			{#if pendingCaptures.count > 0}
				<a
					href="/app/pending"
					class="pill pill-warn pending-badge"
					data-testid="pending-captures-link"
				>
					Pending Captures ({pendingCaptures.count})
				</a>
			{/if}
			<button
				type="button"
				class="btn btn-secondary signout"
				data-testid="logout-button"
				onclick={onLogout}
				disabled={busy}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					aria-hidden="true"
				>
					<path d="M9 21H6a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3" />
					<path d="M16 17l5-5-5-5M21 12H9" />
				</svg>
				{busy ? 'Signing out…' : 'Sign out'}
			</button>
		</div>

		{#if logoutError}
			<p class="error pill pill-danger" data-testid="logout-error">
				{logoutError}
			</p>
		{/if}
		{#if headerTaps >= 5}
			<p class="admin-entry">
				<a href="/app/admin/logs" data-testid="admin-link">Admin - logs</a>
				<span class="sep" aria-hidden="true">·</span>
				<a href="/app/admin/invites" data-testid="admin-invites-link">
					Admin - invitations
				</a>
				<span class="sep" aria-hidden="true">·</span>
				<a href="/app/admin/system" data-testid="admin-system-link">
					Admin - system
				</a>
			</p>
		{/if}
	</header>

	<section class="capture-section rise" data-testid="capture-section">
		<ActiveCapture
			ingest={ingestApi}
			oningest={onIngest}
			pending={pendingCaptures}
			{online}
		/>
	</section>
</main>

<style>
	.app-home {
		max-inline-size: 46rem;
		display: grid;
		gap: var(--space-6);
	}
	.appbar {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--space-3) var(--space-4);
		padding-block: var(--space-1) var(--space-4);
		border-block-end: 1px solid var(--border-hairline);
	}
	.appbar-brand {
		display: flex;
		align-items: baseline;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.wordmark {
		font: inherit;
		font-size: var(--fs-22);
		font-weight: 700;
		letter-spacing: -0.02em;
		color: var(--fg);
		background: transparent;
		border: 0;
		padding: 0;
		margin: 0;
		cursor: default;
	}
	.identity {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		font-size: var(--fs-13);
		color: var(--fg-muted);
	}
	.identity-dot {
		inline-size: 6px;
		block-size: 6px;
		border-radius: 50%;
		background: var(--success);
		box-shadow: 0 0 8px -1px var(--success);
	}
	.identity-label {
		color: var(--fg-subtle);
	}
	.identity code {
		color: var(--accent);
	}
	.appbar-actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		margin-inline-start: auto;
	}
	.pending-badge {
		text-transform: none;
		letter-spacing: normal;
		font-weight: 500;
	}
	.pending-badge:hover {
		color: var(--warn);
		background: var(--warn-soft);
		border-color: var(--warn);
	}
	.signout {
		min-block-size: 40px;
	}
	.signout svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.error {
		flex-basis: 100%;
		padding: 0.5rem 0.8rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
	}
	.admin-entry {
		flex-basis: 100%;
		margin: 0;
	}
	.admin-entry a {
		color: var(--fg-subtle);
		font-size: var(--fs-13);
		text-decoration: underline;
	}
	.admin-entry a:hover {
		color: var(--fg-muted);
	}
	.admin-entry .sep {
		color: var(--fg-subtle);
		font-size: var(--fs-13);
	}
	.capture-section {
		display: grid;
		gap: var(--space-4);
	}

	@media (max-width: 480px) {
		.appbar-actions {
			margin-inline-start: 0;
			flex: 1 1 100%;
			justify-content: flex-end;
		}
	}
</style>
