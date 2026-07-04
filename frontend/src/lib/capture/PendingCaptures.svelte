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
	<p class="empty" data-testid="pending-captures-empty">
		No pending captures — offline submissions will appear here for review when back online.
	</p>
{:else}
	<ul class="queue" data-testid="pending-captures-list">
		{#each store.items as capture (capture.id)}
			<li class="row" data-testid={`pending-capture-item-${capture.id}`}>
				<p class="meta">Captured offline {new Date(capture.createdAt).toLocaleString()}</p>
				<textarea
					data-testid="pending-capture-text"
					value={textFor(capture)}
					oninput={(e) => {
						edits[capture.id] = e.currentTarget.value;
					}}
					rows="3"
					disabled={submitting === capture.id}
				></textarea>
				<button
					type="button"
					data-testid="pending-capture-submit"
					onclick={() => onConfirm(capture)}
					disabled={submitting === capture.id}
				>
					{submitting === capture.id ? 'Submitting…' : 'Submit'}
				</button>
				{#if errors[capture.id]}
					<p class="error" data-testid="pending-capture-error">{errors[capture.id]}</p>
				{/if}
			</li>
		{/each}
	</ul>
{/if}

<style>
	.queue {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: 0.75rem;
	}
	.row {
		display: grid;
		gap: 0.4rem;
		padding: 0.75rem 0.9rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #11141b;
	}
	.meta {
		margin: 0;
		font-size: 0.8rem;
		color: #9aa3b2;
	}
	textarea {
		resize: vertical;
		font: inherit;
		color: #e6e8ec;
		background: #0b0d12;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		padding: 0.5rem;
	}
	button {
		justify-self: start;
		padding: 0.45rem 0.9rem;
		font-size: 0.95rem;
		border: 1px solid #7ab7ff;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #7ab7ff;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	.error {
		margin: 0;
		font-size: 0.85rem;
		color: #ff7a7a;
	}
	.empty {
		margin: 0;
		padding: 0.75rem 0.9rem;
		border: 1px dashed #2a2f3a;
		border-radius: 0.5rem;
		background: #11141b;
		color: #9aa3b2;
		font-size: 0.95rem;
	}
</style>
