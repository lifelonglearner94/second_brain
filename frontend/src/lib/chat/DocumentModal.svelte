<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import type { Braindump } from '$lib/api/client';

	type BraindumpApi = {
		getBraindump(id: number): Promise<Braindump>;
		editBraindump(id: number, verbatim: string): Promise<Braindump>;
	};

	let { braindumpId, api, onClose }: { braindumpId: number; api: BraindumpApi; onClose: () => void } =
		$props();

	type Status = 'loading' | 'ready' | 'error';

	let status = $state<Status>('loading');
	let braindump = $state<Braindump | null>(null);
	let errorText = $state<string | null>(null);
	let viewRaw = $state(false);

	let editing = $state(false);
	let editText = $state('');
	let saving = $state(false);
	let editError = $state<string | null>(null);
	let saved = $state(false);
	let savedTimer: ReturnType<typeof setTimeout> | null = null;

	onDestroy(() => {
		if (savedTimer !== null) clearTimeout(savedTimer);
	});

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

	function startEdit() {
		if (!braindump) return;
		editText = braindump.verbatim;
		editError = null;
		saved = false;
		editing = true;
	}

	function cancelEdit() {
		editing = false;
		editError = null;
	}

	async function saveEdit() {
		if (!braindump || saving) return;
		saving = true;
		editError = null;
		try {
			const res = await api.editBraindump(braindump.id, editText);
			braindump = res;
			editing = false;
			viewRaw = false;
			saved = true;
			if (savedTimer !== null) clearTimeout(savedTimer);
			savedTimer = setTimeout(() => {
				saved = false;
				savedTimer = null;
			}, 1500);
		} catch (e) {
			void e;
			editError = 'Could not save the correction.';
			editing = true;
		} finally {
			saving = false;
		}
		await tick();
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
				{#if editing}
					<textarea
						data-testid="document-modal-edit-input"
						bind:value={editText}
						rows="6"
						class="document-modal-edit-input"
					></textarea>
					{#if editError}
						<p data-testid="document-modal-edit-error" class="document-modal-error">{editError}</p>
					{/if}
					<div class="document-modal-edit-actions">
						<button
							type="button"
							data-testid="document-modal-save"
							onclick={saveEdit}
							disabled={saving}
						>
							{saving ? 'Saving…' : 'Save correction'}
						</button>
						<button
							type="button"
							data-testid="document-modal-cancel"
							onclick={cancelEdit}
							disabled={saving}
						>
							Cancel
						</button>
					</div>
				{:else}
					<div class="document-modal-controls">
						<button
							type="button"
							data-testid="document-modal-toggle-raw"
							onclick={toggleRaw}
						>
							{viewRaw ? 'Show Cleaned' : 'View Raw'}
						</button>
						<button type="button" data-testid="document-modal-edit" onclick={startEdit}>
							Edit
						</button>
					</div>
					{#if saved}
						<p data-testid="document-modal-saved" class="document-modal-saved">Saved</p>
					{/if}
					{#if viewRaw}
						<p data-testid="document-modal-verbatim" class="document-modal-text verbatim">
							{braindump.verbatim}
						</p>
					{:else}
						<p data-testid="document-modal-cleaned" class="document-modal-text cleaned">
							{braindump.cleaned}
						</p>
					{/if}
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
	.document-modal-controls {
		display: flex;
		gap: 0.5rem;
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
	.document-modal-edit-actions {
		display: flex;
		gap: 0.5rem;
	}
	.document-modal-edit-input {
		inline-size: 100%;
		font-family: inherit;
		font-size: 0.9rem;
		line-height: 1.5;
		background: #0d0f15;
		color: #e6e8ec;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		padding: 0.5rem;
		resize: vertical;
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
	.document-modal-saved {
		color: #7ad19a;
		font-size: 0.8rem;
		margin: 0;
	}
</style>
