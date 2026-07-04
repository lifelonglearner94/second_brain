<script lang="ts">
	import type { HousekeepingStore } from '$lib/state/housekeeping.svelte';

	let { store }: { store: HousekeepingStore } = $props();
</script>

<section data-testid="housekeeping-queue" aria-label="Housekeeping Queue">
	<h2>Housekeeping Queue</h2>

	{#if store.status === 'idle' || store.status === 'loading'}
		<p data-testid="housekeeping-loading" class="state">Loading merge suggestions…</p>
	{:else if store.status === 'error'}
		<p data-testid="housekeeping-error" class="state error">{store.error}</p>
	{:else if store.items.length === 0}
		<p data-testid="housekeeping-empty" class="state">No merge suggestions pending.</p>
	{:else}
		<ol class="suggestions" data-testid="housekeeping-list">
			{#each store.items as item (item.id + '-' + item.kind)}
				<li class="suggestion" data-testid={`housekeeping-item-${item.id}-${item.kind}`}>
					<p class="pair">
						<span class="left">{item.leftLabel}</span>
						<span class="arrow" aria-hidden="true">↔</span>
						<span class="right">{item.rightLabel}</span>
					</p>
					<p class="meta">
						<span class="kind" data-kind={item.kind}>{item.kind === 'concept' ? 'Concept' : 'Type'}</span>
						<span class="similarity">Similarity {item.similarity.toFixed(2)}</span>
					</p>
					<button
						type="button"
						data-testid={`housekeeping-merge-${item.id}-${item.kind}`}
						onclick={() => store.confirmMerge(item.id, item.kind)}
					>
						Merge
					</button>
				</li>
			{/each}
		</ol>
	{/if}
</section>

<style>
	section {
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
	}
	h2 {
		margin: 0 0 0.75rem;
		font-size: clamp(1.25rem, 3vw, 1.5rem);
	}
	.state {
		color: #9aa3b2;
	}
	.state.error {
		color: #ff7a7a;
	}
	.suggestions {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 0.6rem;
	}
	.suggestion {
		padding: 0.6rem 0.75rem;
		border: 1px solid #1f242e;
		border-radius: 0.5rem;
		background: #11141b;
		display: grid;
		gap: 0.35rem;
	}
	.pair {
		margin: 0;
		display: flex;
		gap: 0.4rem;
		align-items: baseline;
		flex-wrap: wrap;
	}
	.arrow {
		color: #6b7280;
	}
	.meta {
		margin: 0;
		display: flex;
		gap: 0.75rem;
		font-size: 0.8rem;
		color: #9aa3b2;
	}
	.kind {
		font-family: monospace;
		text-transform: uppercase;
	}
	.kind[data-kind='type'] {
		color: #7ab7ff;
	}
	.kind[data-kind='concept'] {
		color: #b7a7ff;
	}
	button {
		justify-self: start;
		padding: 0.4rem 0.9rem;
		font-size: 0.9rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	button:hover {
		border-color: #7ab7ff;
	}
</style>
