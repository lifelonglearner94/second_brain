<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import { endorsementQueue } from '$lib/state/endorsement-queue.svelte';
	import { graphStore } from '$lib/state/graph.svelte';
	import { createIdb } from '$lib/state/idb';
	import EndorsementQueue from '$lib/endorsement/EndorsementQueue.svelte';

	let labelMap = $state<Map<string, string>>(new Map());

	function labelFor(conceptId: number): string | null {
		return labelMap.get(String(conceptId)) ?? null;
	}

	async function onApproveConnection(proposal: { id: number }): Promise<void> {
		await endorsementQueue.approve(proposal.id);
	}

	onMount(() => {
		(async () => {
			await endorsementQueue.refresh();
			try {
				await graphStore.loadFromNetworkOrCache(apiClient, createIdb());
				labelMap = new Map(
					graphStore.snapshot?.concepts.map(
						(c) => [String(c.id), c.label] as const
					) ?? []
				);
			} catch {
				// The queue still renders with numeric concept ids if the
				// Spatial View-Graph can't be loaded (e.g. offline).
			}
		})();
	});
</script>

<main class="page queue-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="endorsement-back">
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
		<h1>Endorsement Queue</h1>
		<p class="tagline">
			Approve the connections chat inferred. Each proposal shows the evidence it
			rests on before you decide.
		</p>
	</header>

	<section class="body rise">
		{#if endorsementQueue.status === 'loading' && endorsementQueue.pending.length === 0}
			<div class="state card" data-testid="endorsement-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Loading proposals…</span>
			</div>
		{:else if endorsementQueue.status === 'error'}
			<p class="state error pill pill-danger" data-testid="endorsement-error">
				{endorsementQueue.error}
			</p>
		{:else if endorsementQueue.pending.length === 0}
			<div class="state card empty" data-testid="endorsement-empty">
				<div class="empty-mark" aria-hidden="true">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.5"
					>
						<path d="M4 12l5 5L20 6" />
					</svg>
				</div>
				<p>No chat-inferred connections awaiting your endorsement.</p>
			</div>
		{:else}
			<EndorsementQueue
				proposals={endorsementQueue.pending}
				{labelFor}
				{onApproveConnection}
			/>
		{/if}
	</section>

	<p class="graph-summary" data-testid="spatial-graph-summary">
		<span class="eyebrow">Spatial View-Graph</span>
		<span class="summary-count">
			<code class="mono" data-testid="spatial-graph-edge-count"
				>{graphStore.data.links.length}</code
			>
			edges
		</span>
	</p>
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
		flex-direction: column;
		text-align: center;
		gap: var(--space-3);
		padding: var(--space-8);
	}
	.empty-mark {
		display: grid;
		place-items: center;
		inline-size: 2.5rem;
		block-size: 2.5rem;
		color: var(--success);
		background: var(--success-soft);
		border: 1px solid rgba(122, 209, 154, 0.3);
		border-radius: var(--radius-md);
	}
	.empty-mark svg {
		inline-size: 1.3rem;
		block-size: 1.3rem;
	}
	.state.empty p {
		color: var(--fg-muted);
	}
	.error {
		padding: 0.6rem 0.85rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
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
	.graph-summary {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
		padding: var(--space-3) var(--space-4);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		background: var(--surface-glass);
		font-size: var(--fs-13);
	}
	.summary-count {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		color: var(--fg-muted);
	}
	.summary-count code {
		color: var(--accent);
		font-size: var(--fs-16);
		font-weight: 600;
	}
</style>
