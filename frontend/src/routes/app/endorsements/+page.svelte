<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import { endorsementQueue } from '$lib/endorsement/queue';
	import { spatialGraph } from '$lib/state/spatial-graph.svelte';
	import EndorsementQueue from '$lib/endorsement/EndorsementQueue.svelte';

	let labelMap = $state<Map<string, string>>(new Map());

	function labelFor(conceptId: number): string | null {
		return labelMap.get(String(conceptId)) ?? null;
	}

	async function onApproveConnection(proposal: {
		id: number;
	}): Promise<void> {
		await endorsementQueue.approve(proposal.id);
	}

	onMount(() => {
		(async () => {
			await endorsementQueue.refresh();
			try {
				const snapshot = await apiClient.getGraph();
				spatialGraph.loadSnapshot(snapshot);
				labelMap = new Map(
					snapshot.concepts.map((c) => [String(c.id), c.label] as const)
				);
			} catch {
				// The queue still renders with numeric concept ids if the
				// Spatial View-Graph can't be loaded (e.g. offline).
			}
		})();
	});
</script>

<main>
	<header>
		<h1>Endorsement Queue</h1>
		<p class="tagline">
			Approve the connections chat inferred. Each proposal shows the evidence it
			rests on before you decide.
		</p>
	</header>

	{#if endorsementQueue.status === 'loading' && endorsementQueue.pending.length === 0}
		<p class="state" data-testid="endorsement-loading">Loading proposals…</p>
	{:else if endorsementQueue.status === 'error'}
		<p class="state error" data-testid="endorsement-error">{endorsementQueue.error}</p>
	{:else if endorsementQueue.pending.length === 0}
		<p class="state" data-testid="endorsement-empty">
			No chat-inferred connections awaiting your endorsement.
		</p>
	{:else}
		<EndorsementQueue
			proposals={endorsementQueue.pending}
			{labelFor}
			onApproveConnection={onApproveConnection}
		/>
	{/if}

	<p class="graph-summary" data-testid="spatial-graph-summary">
		Spatial View-Graph: <code data-testid="spatial-graph-edge-count">
			{spatialGraph.data.links.length}
		</code>
		edges
	</p>

	<p><a href="/app" data-testid="endorsement-back">Back to the Spatial View-Graph</a></p>
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
	h1 {
		margin: 0 0 0.25rem;
		font-size: clamp(1.5rem, 4vw, 2rem);
	}
	.tagline {
		margin: 0 0 1.5rem;
		color: #9aa3b2;
	}
	.state {
		color: #9aa3b2;
	}
	.error {
		color: #ff7a7a;
	}
	.graph-summary {
		margin: 1.5rem 0 0.5rem;
		color: #9aa3b2;
		font-size: 0.85rem;
	}
	code {
		font-family: monospace;
		color: #7ab7ff;
	}
	a {
		color: #7ab7ff;
	}
</style>
