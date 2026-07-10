<script lang="ts">
	import {
		parseAnswerCitations,
		type AnswerSegment
	} from '$lib/chat/citations';
	import type { Braindump, ChatResponse } from '$lib/api/client';
	import DocumentModal from './DocumentModal.svelte';

	type ChatApi = {
		chat(query: string): Promise<ChatResponse>;
		getBraindump(id: number): Promise<Braindump>;
		editBraindump(id: number, verbatim: string): Promise<Braindump>;
	};

	let { api, online = true }: { api: ChatApi; online?: boolean } = $props();

	type Status = 'idle' | 'loading' | 'error';

	let query = $state('');
	let status = $state<Status>('idle');
	let response = $state<ChatResponse | null>(null);
	let segments = $state<AnswerSegment[]>([]);
	let errorText = $state<string | null>(null);
	let openCitationId = $state<number | null>(null);

	const EXPLICIT_SILENCE =
		'I cannot find graph-supported evidence to answer this.';

	async function onSubmit() {
		if (!online) return;
		if (query.trim().length === 0) return;
		status = 'loading';
		response = null;
		segments = [];
		errorText = null;
		openCitationId = null;
		try {
			const res = await api.chat(query);
			response = res;
			segments = res.silent ? [] : parseAnswerCitations(res.answer);
			status = 'idle';
		} catch (e) {
			errorText = e instanceof Error ? e.message : String(e);
			status = 'error';
		}
	}

	function openCitation(braindumpId: number) {
		openCitationId = braindumpId;
	}

	function closeCitation() {
		openCitationId = null;
	}
</script>

<section class="chat-surface" data-testid="chat-surface">
	{#if !online}
		<div class="notice pill pill-warn" data-testid="chat-offline">
			Chat unavailable offline - connect to use the backend LLM.
		</div>
	{/if}

	<form
		class="composer"
		onsubmit={(e) => {
			e.preventDefault();
			void onSubmit();
		}}
	>
		<label class="eyebrow composer-label" for="chat-query"
			>Ask your second brain</label
		>
		<div class="composer-row">
			<input
				id="chat-query"
				class="input composer-input"
				data-testid="chat-query-input"
				type="text"
				bind:value={query}
				placeholder="What is on your mind?"
				autocomplete="off"
				disabled={!online}
			/>
			<button
				type="submit"
				class="btn btn-primary composer-submit"
				data-testid="chat-submit"
				disabled={!online}
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
				Ask
			</button>
		</div>
	</form>

	{#if status === 'loading'}
		<div class="answer-card card loading" data-testid="chat-loading">
			<span class="dot-pulse" aria-hidden="true"></span>
			<span>Synthesizing over your graph…</span>
		</div>
	{:else if status === 'error'}
		<div class="answer-card card error" data-testid="chat-error">
			<span class="answer-error-label">Could not answer</span>
			<span class="answer-error-detail">{errorText}</span>
		</div>
	{:else if response}
		{#if response.silent}
			<div class="answer-card card silence" role="status">
				<div class="silence-mark" aria-hidden="true">
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="1.6"
						stroke-linecap="round"
					>
						<path d="M5 12h14" />
					</svg>
				</div>
				<p class="silence-text" data-testid="chat-explicit-silence">
					{EXPLICIT_SILENCE}
				</p>
			</div>
		{:else}
			<div class="answer-card card" data-testid="chat-answer">
				<p class="answer-text">
					{#each segments as seg}
						{#if seg.kind === 'text'}
							{seg.text}
						{:else}
							<button
								type="button"
								class="citation-chip"
								data-testid="chat-citation-chip"
								data-braindump-id={seg.braindumpId}
								aria-label={`Open source ${seg.index}`}
								onclick={() => openCitation(seg.braindumpId)}
							>
								[{seg.index}]
							</button>
						{/if}
					{/each}
				</p>
			</div>
		{/if}
	{/if}

	{#if openCitationId !== null}
		<DocumentModal braindumpId={openCitationId} {api} onClose={closeCitation} />
	{/if}
</section>

<style>
	.chat-surface {
		max-inline-size: 46rem;
		margin-inline: auto;
		width: 100%;
		display: grid;
		gap: var(--space-4);
	}
	.notice {
		padding: 0.6rem 0.85rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
		line-height: 1.45;
	}
	.composer {
		display: grid;
		gap: var(--space-2);
	}
	.composer-label {
		color: var(--fg-subtle);
	}
	.composer-row {
		display: flex;
		gap: var(--space-2);
		flex-wrap: wrap;
	}
	.composer-input {
		flex: 1 1 18rem;
		min-block-size: 48px;
	}
	.composer-submit {
		min-block-size: 48px;
	}
	.composer-submit svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.composer-submit:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.composer-input:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.answer-card {
		padding: var(--space-5);
		animation: rise var(--dur-3) var(--ease) both;
	}
	.answer-text {
		line-height: var(--lh-read);
		font-size: var(--fs-16);
		color: var(--fg);
	}
	.loading {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		color: var(--fg-muted);
		font-size: var(--fs-14);
		padding: var(--space-4) var(--space-5);
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
	.error {
		display: grid;
		gap: 0.2rem;
		border-color: var(--danger-border);
	}
	.answer-error-label {
		color: var(--danger);
		font-weight: 600;
		font-size: var(--fs-14);
	}
	.answer-error-detail {
		color: var(--fg-muted);
		font-size: var(--fs-13);
	}

	.silence {
		display: grid;
		gap: var(--space-3);
		justify-items: start;
		border-color: var(--warn-border);
		background: var(--warn-soft), var(--bg-elevated);
	}
	.silence-mark {
		display: grid;
		place-items: center;
		inline-size: 2rem;
		block-size: 2rem;
		color: var(--warn);
		background: rgba(240, 198, 116, 0.1);
		border: 1px solid var(--warn-border);
		border-radius: var(--radius-md);
	}
	.silence-mark svg {
		inline-size: 1.1rem;
		block-size: 1.1rem;
	}
	.silence-text {
		font-size: var(--fs-16);
		color: var(--warn);
		font-weight: 500;
		line-height: var(--lh-body);
	}

	.citation-chip {
		display: inline-flex;
		align-items: center;
		padding: 0 0.35rem;
		margin: 0 0.1rem;
		font-family: var(--font-mono);
		font-size: 0.8em;
		font-weight: 500;
		color: var(--accent);
		background: var(--accent-soft);
		border: 1px solid var(--border-accent);
		border-radius: var(--radius-sm);
		cursor: pointer;
		vertical-align: baseline;
		transition:
			background var(--dur-1) var(--ease),
			border-color var(--dur-1) var(--ease),
			transform var(--dur-1) var(--ease);
	}
	.citation-chip:hover {
		background: rgba(122, 183, 255, 0.22);
		border-color: var(--accent);
		transform: translateY(-1px);
	}
</style>
