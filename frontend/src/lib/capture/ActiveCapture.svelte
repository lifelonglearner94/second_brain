<script lang="ts">
	import { ActiveCaptureStore } from '$lib/capture/active-capture.svelte';
	import { buildSttSources } from '$lib/capture/stt';
	import type { IngestApi, IngestResponse } from '$lib/capture/ingest';
	import type { PendingCapturesStore } from '$lib/state/pending-captures.svelte';

	type Props = {
		ingest: IngestApi;
		deepgramApiKey?: string;
		oningest?: (res: IngestResponse) => void;
		pending: PendingCapturesStore;
		online?: boolean;
	};

	let {
		ingest,
		deepgramApiKey,
		oningest,
		pending,
		online = true
	}: Props = $props();

	const store = new ActiveCaptureStore();
	let busy = $state(false);

	const webSpeechAvailable =
		typeof window !== 'undefined' &&
		(window.SpeechRecognition !== undefined ||
			window.webkitSpeechRecognition !== undefined);

	async function onRecord() {
		if (store.status === 'listening') {
			await store.stopStt();
			return;
		}
		busy = true;
		store.error = null;
		try {
			const { primary, fallback } = await buildSttSources({
				deepgramApiKey,
				webSpeechAvailable
			});
			if (!primary) {
				store.error = 'No STT source available — type instead.';
				store.status = 'error';
				return;
			}
			await store.startCaptureWithFallback(primary, fallback);
		} catch (e) {
			store.error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	async function onSubmit() {
		busy = true;
		store.error = null;
		try {
			const outcome = await store.submit(ingest, online, pending);
			if (outcome.kind === 'submitted') {
				oningest?.(outcome.res);
			}
		} catch (e) {
			store.error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	let canSubmit = $derived(
		store.text.trim().length > 0 && store.status !== 'submitting'
	);
</script>

<section class="active-capture" data-testid="active-capture" aria-live="polite">
	<textarea
		data-testid="active-capture-text"
		bind:value={store.text}
		placeholder="Speak or type your thought…"
		rows="3"></textarea>

	<div class="controls">
		{#if deepgramApiKey || webSpeechAvailable}
			<button
				type="button"
				data-testid="record-button"
				onclick={onRecord}
				disabled={busy || store.status === 'submitting'}
			>
				{store.status === 'listening' ? '⏹ Stop' : '🎤 Record'}
			</button>
		{/if}
		<button
			type="button"
			data-testid="submit-button"
			onclick={onSubmit}
			disabled={!canSubmit || busy}
		>
			{store.status === 'submitting' ? 'Submitting…' : 'Submit'}
		</button>
	</div>

	{#if store.sttSourceLabel}
		<p class="source" data-testid="stt-source">
			STT: {store.sttSourceLabel === 'deepgram'
				? 'Deepgram Nova-3'
				: 'Web Speech (offline)'}
		</p>
	{/if}
	{#if store.status === 'queued'}
		<p class="queued" data-testid="active-capture-queued">
			Saved offline — review in Pending Captures when back online.
		</p>
	{/if}
	{#if store.error}
		<p class="error" data-testid="active-capture-error">{store.error}</p>
	{/if}
</section>

<style>
	.active-capture {
		display: flex;
		flex-direction: column;
		gap: 0.5rem;
		padding: 0.75rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #11141c;
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
	.controls {
		display: flex;
		gap: 0.5rem;
	}
	button {
		padding: 0.45rem 0.9rem;
		font-size: 0.95rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	.source {
		margin: 0;
		font-size: 0.8rem;
		color: #9aa3b2;
	}
	.queued {
		margin: 0;
		font-size: 0.85rem;
		color: #f0c674;
	}
	.error {
		margin: 0;
		font-size: 0.85rem;
		color: #ff7a7a;
	}
</style>
