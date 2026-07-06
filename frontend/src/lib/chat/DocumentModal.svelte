<script lang="ts">
	import { onMount, onDestroy, tick } from 'svelte';
	import type { Braindump } from '$lib/api/client';

	type BraindumpApi = {
		getBraindump(id: number): Promise<Braindump>;
		editBraindump(id: number, verbatim: string): Promise<Braindump>;
	};

	let {
		braindumpId,
		api,
		onClose
	}: { braindumpId: number; api: BraindumpApi; onClose: () => void } = $props();

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

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape' && !saving) {
			e.preventDefault();
			onClose();
		}
	}

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

<svelte:window onkeydown={onKeydown} />

<div
	class="document-modal-overlay"
	role="dialog"
	aria-modal="true"
	aria-label="Document Modal"
>
	<div class="document-modal scale-in">
		<header class="document-modal-header">
			<div class="document-modal-title">
				<span class="eyebrow">Document</span>
			</div>
			<button
				type="button"
				class="btn btn-ghost document-modal-close"
				data-testid="document-modal-close"
				onclick={onClose}
				aria-label="Close"
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					stroke-linecap="round"
					aria-hidden="true"
				>
					<path d="M6 6l12 12M18 6 6 18" />
				</svg>
			</button>
		</header>

		{#if status === 'loading'}
			<div class="document-modal-status" data-testid="document-modal-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Loading braindump…</span>
			</div>
		{:else if status === 'error'}
			<p class="document-modal-error" data-testid="document-modal-error">
				{errorText}
			</p>
		{:else if braindump}
			<div class="document-modal-body">
				{#if editing}
					<textarea
						class="textarea document-modal-edit-input"
						data-testid="document-modal-edit-input"
						bind:value={editText}
						rows="6"
					></textarea>
					{#if editError}
						<p
							class="document-modal-error"
							data-testid="document-modal-edit-error"
						>
							{editError}
						</p>
					{/if}
					<div class="document-modal-edit-actions">
						<button
							type="button"
							class="btn btn-primary"
							data-testid="document-modal-save"
							onclick={saveEdit}
							disabled={saving}
						>
							{saving ? 'Saving…' : 'Save correction'}
						</button>
						<button
							type="button"
							class="btn btn-secondary"
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
							class="btn btn-ghost"
							data-testid="document-modal-toggle-raw"
							onclick={toggleRaw}
						>
							{viewRaw ? 'Show Cleaned' : 'View Raw'}
						</button>
						<button
							type="button"
							class="btn btn-ghost"
							data-testid="document-modal-edit"
							onclick={startEdit}
						>
							<svg
								viewBox="0 0 24 24"
								fill="none"
								stroke="currentColor"
								stroke-width="1.8"
								aria-hidden="true"
							>
								<path d="M4 20h4l11-11-4-4L4 16v4Z" />
								<path d="M14 6l4 4" />
							</svg>
							Edit
						</button>
						{#if saved}
							<span
								class="pill pill-success saved-pill"
								data-testid="document-modal-saved"
							>
								Saved
							</span>
						{/if}
					</div>
					{#if viewRaw}
						<p
							class="document-modal-text verbatim"
							data-testid="document-modal-verbatim"
						>
							{braindump.verbatim}
						</p>
					{:else}
						<p
							class="document-modal-text cleaned"
							data-testid="document-modal-cleaned"
						>
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
		z-index: var(--z-modal);
		display: grid;
		place-items: center;
		padding: var(--space-4);
		background: rgba(4, 5, 8, 0.66);
		backdrop-filter: blur(6px) saturate(120%);
		-webkit-backdrop-filter: blur(6px) saturate(120%);
	}
	.document-modal {
		max-inline-size: 42rem;
		inline-size: 100%;
		max-block-size: 82dvh;
		overflow: auto;
		background: var(--bg-elevated);
		color: var(--fg);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-xl);
		box-shadow: var(--shadow-modal);
	}
	.document-modal-header {
		position: sticky;
		top: 0;
		display: flex;
		align-items: center;
		justify-content: space-between;
		padding: var(--space-3) var(--space-4);
		background: rgba(17, 20, 28, 0.85);
		border-block-end: 1px solid var(--border-hairline);
		backdrop-filter: blur(8px);
		-webkit-backdrop-filter: blur(8px);
	}
	.document-modal-close {
		min-block-size: 36px;
		min-inline-size: 36px;
		block-size: 36px;
		inline-size: 36px;
		padding: 0;
	}
	.document-modal-close svg {
		inline-size: 1.1rem;
		block-size: 1.1rem;
	}
	.document-modal-body {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
		padding: var(--space-6) var(--space-6) var(--space-8);
	}
	.document-modal-controls {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.document-modal-controls .btn-ghost svg {
		inline-size: 1rem;
		block-size: 1rem;
	}
	.saved-pill {
		text-transform: uppercase;
		letter-spacing: var(--tracking-label);
		font-weight: 600;
		font-size: var(--fs-12);
		animation: fade var(--dur-2) var(--ease) both;
	}
	.document-modal-edit-input {
		inline-size: 100%;
		font-family: inherit;
		font-size: var(--fs-16);
		line-height: var(--lh-body);
	}
	.document-modal-edit-actions {
		display: flex;
		gap: var(--space-2);
	}
	.document-modal-text {
		margin: 0;
		white-space: pre-wrap;
		line-height: var(--lh-read);
		font-size: var(--fs-18);
		color: var(--fg);
	}
	.document-modal-text.verbatim {
		color: var(--fg-muted);
		font-style: italic;
		font-size: var(--fs-16);
	}
	.document-modal-status {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-8) var(--space-6);
		color: var(--fg-muted);
		font-size: var(--fs-14);
	}
	.dot-pulse {
		inline-size: 8px;
		block-size: 8px;
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
	.document-modal-error {
		padding: var(--space-6);
		color: var(--danger);
		font-size: var(--fs-14);
	}
</style>
