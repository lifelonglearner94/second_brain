<script lang="ts">
	import {
		parseAnswerCitations,
		type AnswerSegment
	} from '$lib/chat/citations';
	import { composeAnswer, mountCitationChips } from './markdown';
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
	let answerEl = $state<HTMLDivElement | null>(null);

	const answer = $derived(
		segments.length > 0 ? composeAnswer(segments) : { html: '', chips: [] }
	);

	$effect(() => {
		const el = answerEl;
		const { html, chips } = answer;
		if (!el || !html) return;
		mountCitationChips(el, chips, openCitation);
	});

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
				<div class="answer-text" bind:this={answerEl}>
					<!-- eslint-disable-next-line svelte/no-at-html-tags -- sanitized via DOMPurify in composeAnswer (issue #95) -->
					{@html answer.html}
				</div>
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
		word-wrap: break-word;
	}
	.answer-text :global(p),
	.answer-text :global(ul),
	.answer-text :global(ol),
	.answer-text :global(pre),
	.answer-text :global(blockquote),
	.answer-text :global(hr) {
		margin: 0 0 var(--space-3);
	}
	.answer-text :global(p:last-child),
	.answer-text :global(ul:last-child),
	.answer-text :global(ol:last-child),
	.answer-text :global(pre:last-child),
	.answer-text :global(blockquote:last-child),
	.answer-text :global(hr:last-child) {
		margin-bottom: 0;
	}
	.answer-text :global(h1),
	.answer-text :global(h2),
	.answer-text :global(h3),
	.answer-text :global(h4),
	.answer-text :global(h5),
	.answer-text :global(h6) {
		margin: var(--space-4) 0 var(--space-2);
		font-weight: 600;
		line-height: var(--lh-tight);
		color: var(--fg);
		text-wrap: balance;
	}
	.answer-text :global(h1) {
		font-size: var(--fs-22);
	}
	.answer-text :global(h2) {
		font-size: var(--fs-18);
	}
	.answer-text :global(h3) {
		font-size: var(--fs-16);
	}
	.answer-text :global(h4),
	.answer-text :global(h5),
	.answer-text :global(h6) {
		font-size: var(--fs-14);
		color: var(--fg-muted);
	}
	.answer-text :global(h1:first-child),
	.answer-text :global(h2:first-child),
	.answer-text :global(h3:first-child),
	.answer-text :global(h4:first-child),
	.answer-text :global(h5:first-child),
	.answer-text :global(h6:first-child) {
		margin-top: 0;
	}
	.answer-text :global(ul),
	.answer-text :global(ol) {
		padding-left: 1.4rem;
	}
	.answer-text :global(li) {
		margin: var(--space-1) 0;
	}
	.answer-text :global(li::marker) {
		color: var(--fg-subtle);
	}
	.answer-text :global(code) {
		font-family: var(--font-mono);
		font-size: 0.875em;
		color: var(--accent-strong);
		background: var(--bg-sunken);
		padding: 0.1rem 0.35rem;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-hairline);
	}
	.answer-text :global(pre) {
		background: var(--bg-sunken);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		padding: var(--space-3) var(--space-4);
		overflow-x: auto;
	}
	.answer-text :global(pre code) {
		color: var(--fg);
		background: none;
		border: none;
		padding: 0;
		font-size: var(--fs-14);
		line-height: var(--lh-body);
	}
	.answer-text :global(blockquote) {
		border-left: 3px solid var(--border-accent);
		padding: var(--space-1) var(--space-4);
		color: var(--fg-muted);
	}
	.answer-text :global(blockquote p) {
		margin: 0;
	}
	.answer-text :global(strong) {
		font-weight: 600;
	}
	.answer-text :global(em) {
		font-style: italic;
	}
	.answer-text :global(del),
	.answer-text :global(s) {
		color: var(--fg-subtle);
	}
	.answer-text :global(a) {
		color: var(--accent);
		text-decoration: underline;
		text-underline-offset: 2px;
	}
	.answer-text :global(a:hover) {
		color: var(--accent-strong);
	}
	.answer-text :global(hr) {
		border: none;
		border-top: 1px solid var(--border-hairline);
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

	.answer-text :global(.citation-chip) {
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
	.answer-text :global(.citation-chip:hover) {
		background: rgba(122, 183, 255, 0.22);
		border-color: var(--accent);
		transform: translateY(-1px);
	}
</style>
