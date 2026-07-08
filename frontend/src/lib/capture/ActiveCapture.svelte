<script lang="ts">
	import { ActiveCaptureStore } from '$lib/capture/active-capture.svelte';
	import { buildSttSources, describeSttAvailability } from '$lib/capture/stt';
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

	const availability = $derived(
		describeSttAvailability({ deepgramApiKey, webSpeechAvailable })
	);

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

	let listening = $derived(store.status === 'listening');
</script>

<section
	class="active-capture card"
	data-testid="active-capture"
	aria-live="polite"
>
	<div class="composer" class:listening>
		<textarea
			class="textarea composer-input"
			data-testid="active-capture-text"
			bind:value={store.text}
			placeholder="Speak or type your thought…"
			rows="3"
		></textarea>
		{#if listening}
			<span class="live-bars" aria-hidden="true">
				<span></span><span></span><span></span><span></span><span></span>
			</span>
		{/if}
	</div>

	<div class="controls">
		{#if availability.canCaptureVoice}
			<button
				type="button"
				class="btn record"
				class:recording={listening}
				data-testid="record-button"
				onclick={onRecord}
				disabled={busy || store.status === 'submitting'}
				aria-pressed={listening}
			>
				{#if listening}
					<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
						<rect x="6" y="6" width="12" height="12" rx="2.5" />
					</svg>
					<span>Stop</span>
				{:else}
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.8"
						stroke-linecap="round"
						aria-hidden="true"
					>
						<rect x="9" y="3" width="6" height="11" rx="3" />
						<path d="M5 11a7 7 0 0 0 14 0M12 18v3" />
					</svg>
					<span>Record</span>
				{/if}
			</button>
		{/if}
		<button
			type="button"
			class="btn btn-primary submit"
			data-testid="submit-button"
			onclick={onSubmit}
			disabled={!canSubmit || busy}
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
				<path d="M4 12l16-8-6 16-3-7-7-1Z" />
			</svg>
			{store.status === 'submitting' ? 'Submitting…' : 'Submit'}
		</button>
	</div>

	{#if store.sttSourceLabel}
		<p class="meta" data-testid="stt-source">
			<span class="pill pill-muted">
				STT: {store.sttSourceLabel === 'deepgram'
					? 'Deepgram Nova-3'
					: 'Web Speech (offline)'}
			</span>
		</p>
	{/if}
	{#if !availability.canCaptureVoice && availability.reason}
		<p class="meta" data-testid="active-capture-no-voice">
			<span class="pill pill-muted">{availability.reason}</span>
		</p>
	{/if}
	{#if store.status === 'queued'}
		<p class="meta" data-testid="active-capture-queued">
			<span class="pill pill-warn">
				Saved offline — review in Pending Captures when back online.
			</span>
		</p>
	{/if}
	{#if store.error}
		<p class="meta" data-testid="active-capture-error">
			<span class="pill pill-danger">{store.error}</span>
		</p>
	{/if}
</section>

<style>
	.active-capture {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4);
	}
	.composer {
		position: relative;
		display: grid;
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		background: var(--bg-sunken);
		transition:
			border-color var(--dur-1) var(--ease),
			box-shadow var(--dur-1) var(--ease);
	}
	.composer:focus-within {
		border-color: var(--accent);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}
	.composer.listening {
		border-color: var(--border-accent);
		box-shadow:
			0 0 0 3px var(--accent-soft),
			inset 0 0 0 1px var(--accent-soft);
	}
	.composer-input {
		border: 0;
		background: transparent;
		resize: vertical;
		min-block-size: 5.5rem;
	}
	.composer-input:focus {
		box-shadow: none;
		background: transparent;
	}
	.live-bars {
		position: absolute;
		inset-block-end: 0.6rem;
		inset-inline-end: 0.7rem;
		display: inline-flex;
		align-items: flex-end;
		gap: 2px;
		block-size: 14px;
		pointer-events: none;
	}
	.live-bars span {
		inline-size: 3px;
		block-size: 100%;
		background: var(--accent);
		border-radius: 2px;
		transform-origin: bottom;
		animation: bar 900ms var(--ease) infinite;
	}
	.live-bars span:nth-child(1) {
		animation-delay: 0ms;
	}
	.live-bars span:nth-child(2) {
		animation-delay: 120ms;
	}
	.live-bars span:nth-child(3) {
		animation-delay: 240ms;
	}
	.live-bars span:nth-child(4) {
		animation-delay: 360ms;
	}
	.live-bars span:nth-child(5) {
		animation-delay: 480ms;
	}
	@keyframes bar {
		0%,
		100% {
			transform: scaleY(0.35);
		}
		50% {
			transform: scaleY(1);
		}
	}
	.controls {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.record {
		color: var(--fg-muted);
	}
	.record:hover {
		color: var(--fg);
	}
	.record.recording {
		color: var(--danger);
		border-color: var(--danger-border);
		background: var(--danger-soft);
	}
	.record.recording:hover {
		background: rgba(255, 122, 122, 0.18);
	}
	.record svg {
		inline-size: 1.1rem;
		block-size: 1.1rem;
	}
	.submit {
		margin-inline-start: auto;
	}
	.submit svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.meta {
		margin: 0;
	}
	.meta .pill {
		text-transform: none;
		letter-spacing: normal;
		font-weight: 400;
		padding: 0.4rem 0.7rem;
		font-size: var(--fs-13);
		line-height: 1.45;
	}
</style>
