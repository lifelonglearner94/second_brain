<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import { housekeeping } from '$lib/state/housekeeping.svelte';
	import { graphStore } from '$lib/state/graph.svelte';
	import { createIdb } from '$lib/state/idb';
	import HousekeepingQueue from '$lib/housekeeping/HousekeepingQueue.svelte';

	onMount(() => {
		(async () => {
			try {
				await graphStore.loadFromNetworkOrCache(apiClient, createIdb());
			} catch {
				// Offline / no cached snapshot: the queue still renders with
				// numeric concept ids as the label fallback.
			}
			await housekeeping.load();
		})();
	});

	async function onRefresh() {
		await housekeeping.load();
	}
</script>

<main>
	<header>
		<h1>Housekeeping Queue</h1>
		<p class="tagline" data-testid="housekeeping-tagline">
			Semantic housekeeping — confirm concept and type merges the system flagged
			as borderline.
		</p>
		<button
			type="button"
			data-testid="housekeeping-refresh"
			onclick={onRefresh}
			disabled={housekeeping.status === 'loading'}
		>
			{housekeeping.status === 'loading' ? 'Refreshing…' : 'Refresh'}
		</button>
	</header>

	<HousekeepingQueue store={housekeeping} />

	<p>
		<a href="/app" data-testid="housekeeping-back"
			>Back to the Spatial View-Graph</a
		>
	</p>
</main>

<style>
	main {
		max-inline-size: 48rem;
		margin-inline: auto;
		padding: 2rem 1rem;
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
		margin-block-end: 1.5rem;
	}
	h1 {
		margin: 0 0 0.25rem;
		font-size: clamp(1.5rem, 4vw, 2rem);
	}
	.tagline {
		margin: 0 0 0.75rem;
		color: #9aa3b2;
	}
	button {
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
	a {
		color: #7ab7ff;
	}
</style>
