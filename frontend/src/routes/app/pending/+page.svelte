<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import { pendingCaptures } from '$lib/state/pending-captures.svelte';
	import { createIngestApi, type IngestResponse } from '$lib/capture/ingest';
	import PendingCaptures from '$lib/capture/PendingCaptures.svelte';

	let deltaCursor = $state(0);
	const ingestApi = createIngestApi(apiClient, () => deltaCursor);

	function onIngest(res: IngestResponse): void {
		deltaCursor = res.cursor;
	}

	onMount(() => {
		void pendingCaptures.load();
	});
</script>

<main class="page queue-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="pending-back">
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
		<h1>Pending Captures</h1>
		<p class="tagline" data-testid="pending-tagline">
			Offline submissions await review. Offline STT is significantly less
			accurate, so correct any errors, then submit each to ingest it as a
			braindump.
		</p>
	</header>

	<section class="body rise">
		<PendingCaptures
			store={pendingCaptures}
			ingest={ingestApi}
			oningest={onIngest}
		/>
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
		max-inline-size: 38rem;
		line-height: var(--lh-body);
	}
</style>
