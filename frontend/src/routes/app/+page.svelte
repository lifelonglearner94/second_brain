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

	const deepgramApiKey = import.meta.env.VITE_DEEPGRAM_API_KEY as
		string | undefined;
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

<main>
	<header>
		<h1>
			<button
				type="button"
				data-testid="app-title"
				class="title-button"
				onclick={onHeaderTap}>Second Brain</button
			>
		</h1>
		<p class="tagline">
			Signed in as <code data-testid="user-id">{session.userId}</code>
		</p>
		{#if pendingCaptures.count > 0}
			<a
				href="/app/pending"
				class="pending-link"
				data-testid="pending-captures-link"
			>
				Pending Captures ({pendingCaptures.count})
			</a>
		{/if}
		<button
			type="button"
			data-testid="logout-button"
			onclick={onLogout}
			disabled={busy}
		>
			{busy ? 'Signing out…' : 'Sign out'}
		</button>
		{#if logoutError}
			<p data-testid="logout-error" class="error">{logoutError}</p>
		{/if}
		{#if headerTaps >= 5}
			<p class="admin-entry">
				<a href="/app/admin/logs" data-testid="admin-link">Admin — logs</a>
			</p>
		{/if}
	</header>

	<section class="capture-section" data-testid="capture-section">
		<ActiveCapture
			ingest={ingestApi}
			{deepgramApiKey}
			oningest={onIngest}
			pending={pendingCaptures}
			{online}
		/>
	</section>
</main>

<style>
	main {
		margin-inline: auto;
		padding: 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
		background: #0b0d12;
		min-block-size: 100vh;
		box-sizing: border-box;
	}
	header {
		display: flex;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-block-end: 1rem;
	}
	h1 {
		margin: 0;
		font-size: clamp(1.25rem, 3vw, 1.5rem);
	}
	.title-button {
		font: inherit;
		color: inherit;
		background: transparent;
		border: 0;
		padding: 0;
		margin: 0;
		cursor: default;
	}
	.tagline {
		margin: 0;
		color: #9aa3b2;
	}
	code {
		font-family: monospace;
		color: #7ab7ff;
	}
	.pending-link {
		color: #f0c674;
		text-decoration: none;
		font-size: 0.95rem;
		padding: 0.4rem 0.8rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #1a1f2b;
	}
	.pending-link:hover {
		text-decoration: underline;
	}
	.admin-entry {
		margin: 0 0 1.5rem;
	}
	.admin-entry a {
		color: #6b7280;
		font-size: 0.85rem;
		text-decoration: underline;
	}
	button {
		margin-inline-start: auto;
		padding: 0.5rem 1rem;
		font-size: 0.95rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	.capture-section {
		margin-block-end: 1rem;
	}
	.error {
		color: #ff7a7a;
	}
</style>
