<script lang="ts">
	import type { HousekeepingStore } from '$lib/state/housekeeping.svelte';

	let { store }: { store: HousekeepingStore } = $props();
</script>

<section
	class="housekeeping"
	data-testid="housekeeping-queue"
	aria-label="Housekeeping Queue"
>
	<h2>Housekeeping Queue</h2>

	{#if store.status === 'idle' || store.status === 'loading'}
		<div class="state card" data-testid="housekeeping-loading">
			<span class="dot-pulse" aria-hidden="true"></span>
			<span>Loading merge suggestions…</span>
		</div>
	{:else if store.status === 'error'}
		<p class="state error pill pill-danger" data-testid="housekeeping-error">
			{store.error}
		</p>
	{:else if store.items.length === 0}
		<div class="state card empty" data-testid="housekeeping-empty">
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
			<p>No merge suggestions pending.</p>
		</div>
	{:else}
		<ol class="suggestions" data-testid="housekeeping-list">
			{#each store.items as item (item.id + '-' + item.kind)}
				<li
					class="suggestion card"
					data-testid={`housekeeping-item-${item.id}-${item.kind}`}
				>
					<p class="pair">
						<span class="left">{item.leftLabel}</span>
						<span class="arrow" aria-hidden="true">
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="1.6"
							>
								<path d="M7 8l-3 4 3 4M4 12h16M17 8l3 4-3 4" />
							</svg>
						</span>
						<span class="right">{item.rightLabel}</span>
					</p>
					<div class="meta">
						<span class="pill kind" data-kind={item.kind}>
							{item.kind === 'concept' ? 'Concept' : 'Type'}
						</span>
						<span class="similarity">
							<span class="sim-track" aria-hidden="true">
								<span
									class="sim-fill"
									style="width: {Math.round(item.similarity * 100)}%"
								></span>
							</span>
							<span class="sim-text"
								>Similarity {item.similarity.toFixed(2)}</span
							>
						</span>
				</div>
				{#if item.braindumpSnippet}
					<div
						class="provenance"
						data-testid={`housekeeping-provenance-${item.id}-${item.kind}`}
					>
						<span class="provenance-label">From braindump:</span>
						<p class="provenance-text">{item.braindumpSnippet}</p>
					</div>
				{/if}
				<div class="actions">
					<button
						type="button"
						class="btn btn-primary merge"
						data-testid={`housekeeping-merge-${item.id}-${item.kind}`}
						onclick={() => store.confirmMerge(item.id, item.kind)}
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="2"
							stroke-linecap="round"
							stroke-linejoin="round"
							aria-hidden="true"
						>
							<path
								d="M9 7v-3M15 7v-3M9 17v3M15 17v3M7 9h-3M7 15h-3M20 9h-3M20 15h-3"
							/>
							<rect x="7" y="7" width="10" height="10" rx="2" />
						</svg>
						Merge
					</button>
					<button
						type="button"
						class="btn btn-ghost keep-separate"
						data-testid={`housekeeping-reject-${item.id}-${item.kind}`}
						onclick={() => store.rejectMerge(item.id, item.kind)}
					>
						Keep separate
					</button>
				</div>
				</li>
			{/each}
		</ol>
	{/if}
</section>

<style>
	.housekeeping {
		display: grid;
		gap: var(--space-4);
	}
	h2 {
		font-size: var(--fs-22);
		font-weight: 600;
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
	.error {
		padding: 0.6rem 0.85rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
	}
	.suggestions {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-3);
	}
	.suggestion {
		padding: var(--space-4) var(--space-5);
		display: grid;
		gap: var(--space-3);
		transition:
			border-color var(--dur-1) var(--ease),
			box-shadow var(--dur-1) var(--ease);
	}
	.suggestion:hover {
		border-color: var(--border-strong);
	}
	.pair {
		margin: 0;
		display: flex;
		gap: var(--space-3);
		align-items: center;
		flex-wrap: wrap;
		font-size: var(--fs-16);
		color: var(--fg);
		font-weight: 500;
	}
	.arrow {
		display: inline-flex;
		color: var(--fg-subtle);
		flex: 0 0 auto;
	}
	.arrow svg {
		inline-size: 1.25rem;
		block-size: 1.25rem;
	}
	.meta {
		display: flex;
		gap: var(--space-4);
		align-items: center;
		flex-wrap: wrap;
		font-size: var(--fs-13);
		color: var(--fg-muted);
	}
	.kind {
		text-transform: uppercase;
		letter-spacing: var(--tracking-label);
		font-weight: 600;
		font-size: var(--fs-12);
	}
	.kind[data-kind='type'] {
		color: var(--accent);
		background: var(--accent-soft);
		border-color: var(--border-accent);
	}
	.kind[data-kind='concept'] {
		color: var(--concept);
		background: var(--concept-soft);
		border-color: rgba(183, 167, 255, 0.3);
	}
	.similarity {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
	}
	.sim-track {
		inline-size: 5rem;
		block-size: 4px;
		border-radius: var(--radius-pill);
		background: var(--surface-glass-strong);
		overflow: hidden;
	}
	.sim-fill {
		display: block;
		block-size: 100%;
		background: linear-gradient(90deg, var(--accent), var(--accent-strong));
		border-radius: inherit;
		transition: width var(--dur-3) var(--ease);
	}
	.sim-text {
		color: var(--fg-muted);
		font-size: var(--fs-13);
		font-variant-numeric: tabular-nums;
	}
	.merge {
		justify-self: start;
	}
	.merge svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.actions {
		display: flex;
		align-items: center;
		gap: var(--space-2);
	}
	.keep-separate {
		color: var(--fg-muted);
	}
	.provenance {
		display: grid;
		gap: var(--space-1);
		padding: var(--space-2) var(--space-3);
		border-inline-start: 2px solid var(--border-hairline);
		background: var(--surface-glass);
		border-radius: 0 var(--radius-sm) var(--radius-sm) 0;
	}
	.provenance-label {
		font-size: var(--fs-12);
		text-transform: uppercase;
		letter-spacing: var(--tracking-label);
		color: var(--fg-subtle);
		font-weight: 600;
	}
	.provenance-text {
		margin: 0;
		font-size: var(--fs-13);
		color: var(--fg-muted);
		line-height: 1.5;
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
</style>
