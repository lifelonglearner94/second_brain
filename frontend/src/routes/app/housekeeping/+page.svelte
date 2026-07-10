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

<main class="page queue-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="housekeeping-back">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				aria-hidden="true"
			>
				<path d="M15 18l-6-6 6-6" />
			</svg>
			Back to the Spatial View-Graph
		</a>
		<div class="head-row">
			<h1>Housekeeping Queue</h1>
			<button
				type="button"
				class="btn btn-secondary refresh"
				data-testid="housekeeping-refresh"
				onclick={onRefresh}
				disabled={housekeeping.status === 'loading'}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					stroke-linecap="round"
					stroke-linejoin="round"
					aria-hidden="true"
				>
					<path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v5h-5" />
				</svg>
				{housekeeping.status === 'loading' ? 'Refreshing…' : 'Refresh'}
			</button>
		</div>
		<p class="tagline" data-testid="housekeeping-tagline">
			Semantic housekeeping - confirm concept and type merges the system flagged
			as borderline.
		</p>
	</header>

	<section class="body rise">
		<HousekeepingQueue store={housekeeping} />
	</section>
</main>

<style>
	.queue-page {
		max-inline-size: 48rem;
		display: grid;
		gap: var(--space-6);
	}
	.page-head {
		display: grid;
		gap: var(--space-3);
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
	.head-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.page-head h1 {
		font-size: var(--fs-28);
		font-weight: 700;
	}
	.refresh svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.refresh:disabled svg {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.tagline {
		color: var(--fg-muted);
		font-size: var(--fs-14);
		max-inline-size: 38rem;
	}
</style>
