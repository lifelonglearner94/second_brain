<script lang="ts">
	import type { IngestApi, IngestResponse } from '$lib/capture/ingest';
	import type { PendingCapturesStore } from '$lib/state/pending-captures.svelte';
	import type { PendingCapture } from '$lib/state/idb';

	let {
		store,
		ingest,
		oningest
	}: {
		store: PendingCapturesStore;
		ingest: IngestApi;
		oningest?: (res: IngestResponse) => void;
	} = $props();

	let edits = $state<Record<string, string>>({});
	let submitting = $state<string | null>(null);
	let errors = $state<Record<string, string>>({});

	function textFor(c: PendingCapture): string {
		return edits[c.id] ?? c.text;
	}

	async function onConfirm(capture: PendingCapture): Promise<void> {
		const text = textFor(capture);
		submitting = capture.id;
		errors[capture.id] = '';
		try {
			const res = await ingest.ingest(text);
			await store.remove(capture.id);
			delete edits[capture.id];
			oningest?.(res);
		} catch (e) {
			errors[capture.id] = e instanceof Error ? e.message : String(e);
		} finally {
			submitting = null;
		}
	}
</script>

{#if store.items.length === 0}
	<div class="empty card" data-testid="pending-captures-empty">
		<div class="empty-mark" aria-hidden="true">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="1.5"
				stroke-linecap="round"
				stroke-linejoin="round"
			>
				<path d="M12 4v16M4 12h16" />
			</svg>
		</div>
		<p>
			No pending captures — offline submissions will appear here for review when
			back online.
		</p>
	</div>
{:else}
	<ul class="queue" data-testid="pending-captures-list">
		{#each store.items as capture (capture.id)}
			{@const isSubmitting = submitting === capture.id}
			<li
				class="row card"
				class:submitting={isSubmitting}
				data-testid={`pending-capture-item-${capture.id}`}
			>
				<p class="meta">
					<span class="pill pill-warn meta-pill">
						<span class="meta-dot" aria-hidden="true"></span>
						Captured offline
					</span>
					<time class="meta-time mono"
						>{new Date(capture.createdAt).toLocaleString()}</time
					>
				</p>
				<textarea
					class="textarea row-input"
					data-testid="pending-capture-text"
					value={textFor(capture)}
					oninput={(e) => {
						edits[capture.id] = e.currentTarget.value;
					}}
					rows="3"
					disabled={isSubmitting}
				></textarea>
				<div class="row-actions">
					<button
						type="button"
						class="btn btn-primary submit"
						data-testid="pending-capture-submit"
						onclick={() => onConfirm(capture)}
						disabled={isSubmitting}
					>
						{#if isSubmitting}
							<span class="spinner" aria-hidden="true"></span>
							Submitting…
						{:else}
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="2"
								stroke-linecap="round"
								stroke-linejoin="round"
								aria-hidden="true"
							>
								<path d="M4 12l16-8-6 16-3-7-7-1Z" />
							</svg>
							Submit
						{/if}
					</button>
				</div>
				{#if errors[capture.id]}
					<p class="error pill pill-danger" data-testid="pending-capture-error">
						{errors[capture.id]}
					</p>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.empty {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-10) var(--space-6);
		text-align: center;
		color: var(--fg-muted);
		font-size: var(--fs-14);
		border-style: dashed;
	}
	.empty-mark {
		display: grid;
		place-items: center;
		inline-size: 2.5rem;
		block-size: 2.5rem;
		color: var(--fg-subtle);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
	}
	.empty-mark svg {
		inline-size: 1.3rem;
		block-size: 1.3rem;
	}
	.queue {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-3);
	}
	.row {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-5);
		transition:
			border-color var(--dur-1) var(--ease),
			opacity var(--dur-1) var(--ease);
	}
	.row.submitting {
		opacity: 0.7;
	}
	.meta {
		margin: 0;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.meta-pill {
		text-transform: none;
		letter-spacing: normal;
		font-weight: 500;
		font-size: var(--fs-12);
	}
	.meta-dot {
		inline-size: 6px;
		block-size: 6px;
		border-radius: 50%;
		background: var(--warn);
	}
	.meta-time {
		color: var(--fg-subtle);
		font-size: var(--fs-12);
	}
	.row-input {
		min-block-size: 4.5rem;
	}
	.row-actions {
		display: flex;
	}
	.submit {
		justify-self: start;
	}
	.submit svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.spinner {
		inline-size: 0.9rem;
		block-size: 0.9rem;
		border: 2px solid var(--accent-soft);
		border-top-color: var(--accent);
		border-radius: 50%;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}
	.error {
		padding: 0.5rem 0.8rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
		line-height: 1.45;
	}
</style>
