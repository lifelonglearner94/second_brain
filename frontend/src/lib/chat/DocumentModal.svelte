<script lang="ts">
	import { onMount } from 'svelte';
	import type { Braindump } from '$lib/api/client';

	type BraindumpApi = {
		getBraindump(id: number): Promise<Braindump>;
	};

	let { braindumpId, api, onClose }: { braindumpId: number; api: BraindumpApi; onClose: () => void } =
		$props();

	type Status = 'loading' | 'ready' | 'error';

	let status = $state<Status>('loading');
	let braindump = $state<Braindump | null>(null);
	let errorText = $state<string | null>(null);
	let viewRaw = $state(false);

	onMount(() => {
		void (async () => {
			try {
				braindump = await api.getBraindump(braindumpId);
				status = 'ready';
			} catch (e) {
				const message = e instanceof Error ? e.message : String(e);
				errorText = message.includes('404')
					? 'Braindump not found.'
					: 'Could not load this braindump.';
				status = 'error';
			}
		})();
	});

	function toggleRaw() {
		viewRaw = !viewRaw;
	}
</script>

<div class="document-modal-overlay" role="dialog" aria-modal="true" aria-label="Document Modal">
	<div class="document-modal">
		<header class="document-modal-header">
			<span class="document-modal-title">Document</span>
			<button type="button" data-testid="document-modal-close" onclick={onClose}>Close</button>
		</header>

		{#if status === 'loading'}
			<p data-testid="document-modal-loading" class="document-modal-status">Loading braindump…</p>
		{:else if status === 'error'}
			<p data-testid="document-modal-error" class="document-modal-error">{errorText}</p>
		{:else if braindump}
			<div class="document-modal-body">
				<button
					type="button"
					data-testid="document-modal-toggle-raw"
					onclick={toggleRaw}
				>
					{viewRaw ? 'Show Cleaned' : 'View Raw'}
				</button>
				{#if viewRaw}
					<p data-testid="document-modal-verbatim" class="document-modal-text verbatim">
						{braindump.verbatim}
					</p>
				{:else}
					<p data-testid="document-modal-cleaned" class="document-modal-text cleaned">
						{braindump.cleaned}
					</p>
				{/if}
			</div>
		{/if}
	</div>
</div>

<style>
	.document-modal-overlay {
		position: fixed;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background: rgba(11, 13, 18, 0.7);
		z-index: 50;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
	}
	.document-modal {
		max-inline-size: 40rem;
		inline-size: 90%;
		max-block-size: 80vh;
		overflow: auto;
		background: #11141c;
		color: #e6e8ec;
		border: 1px solid #2a2f3a;
		border-radius: 0.6rem;
		padding: 1rem;
	}
	.document-modal-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-block-end: 0.75rem;
	}
	.document-modal-title {
		font-size: 0.9rem;
		color: #9aa3b2;
		letter-spacing: 0.04em;
		text-transform: uppercase;
	}
	.document-modal-header button {
		padding: 0.3rem 0.7rem;
		font-size: 0.85rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	.document-modal-body {
		display: flex;
		flex-direction: column;
		gap: 0.75rem;
	}
	.document-modal-body button {
		align-self: flex-start;
		padding: 0.3rem 0.6rem;
		font-size: 0.8rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #9aa3b2;
		cursor: pointer;
	}
	.document-modal-text {
		margin: 0;
		white-space: pre-wrap;
		line-height: 1.5;
	}
	.document-modal-text.verbatim {
		color: #9aa3b2;
		font-style: italic;
	}
	.document-modal-status {
		color: #9aa3b2;
	}
	.document-modal-error {
		color: #ff7a7a;
	}
</style>
