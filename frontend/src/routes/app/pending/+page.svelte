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

<main>
	<header>
		<h1>Pending Captures</h1>
		<p class="tagline" data-testid="pending-tagline">
			Offline submissions await review. Offline STT is significantly less accurate,
			so correct any errors, then submit each to ingest it as a braindump.
		</p>
	</header>

	<PendingCaptures store={pendingCaptures} ingest={ingestApi} oningest={onIngest} />

	<p><a href="/app" data-testid="pending-back">Back to the Spatial View-Graph</a></p>
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
		margin: 0;
		color: #9aa3b2;
	}
	a {
		color: #7ab7ff;
	}
</style>
