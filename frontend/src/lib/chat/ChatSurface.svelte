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
		<p class="chat-offline" data-testid="chat-offline">
			Chat unavailable offline — connect to use the backend LLM.
		</p>
	{/if}
	<form
		class="chat-form"
		onsubmit={(e) => {
			e.preventDefault();
			void onSubmit();
		}}
	>
		<label class="chat-label" for="chat-query">Ask your second brain</label>
		<input
			id="chat-query"
			class="chat-input"
			data-testid="chat-query-input"
			type="text"
			bind:value={query}
			placeholder="What is on your mind?"
			autocomplete="off"
			disabled={!online}
		/>
		<button
			type="submit"
			class="chat-submit"
			data-testid="chat-submit"
			disabled={!online}>Ask</button
		>
	</form>

	{#if status === 'loading'}
		<p class="chat-status" data-testid="chat-loading">
			Synthesizing over your graph…
		</p>
	{:else if status === 'error'}
		<p class="chat-error" data-testid="chat-error">
			Could not answer: {errorText}
		</p>
	{:else if response}
		{#if response.silent}
			<p class="chat-silence" data-testid="chat-explicit-silence">
				{EXPLICIT_SILENCE}
			</p>
		{:else}
			<p class="chat-answer" data-testid="chat-answer">
				{#each segments as seg}
					{#if seg.kind === 'text'}
						{seg.text}
					{:else}
						<button
							type="button"
							class="chat-citation-chip"
							data-testid="chat-citation-chip"
							data-braindump-id={seg.braindumpId}
							onclick={() => openCitation(seg.braindumpId)}
						>
							[{seg.index}]
						</button>
					{/if}
				{/each}
			</p>
		{/if}
	{/if}

	{#if openCitationId !== null}
		<DocumentModal braindumpId={openCitationId} {api} onClose={closeCitation} />
	{/if}
</section>

<style>
	.chat-surface {
		max-inline-size: 44rem;
		margin-inline: auto;
		padding: 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
	}
	.chat-form {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: center;
		margin-block-end: 1rem;
	}
	.chat-label {
		flex-basis: 100%;
		font-size: 0.85rem;
		color: #9aa3b2;
	}
	.chat-input {
		flex: 1 1 20rem;
		padding: 0.55rem 0.7rem;
		font-size: 1rem;
		color: #e6e8ec;
		background: #11141c;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
	}
	.chat-submit {
		padding: 0.55rem 1rem;
		font-size: 0.95rem;
		color: #e6e8ec;
		background: #1a1f2b;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		cursor: pointer;
	}
	.chat-status {
		color: #9aa3b2;
	}
	.chat-error {
		color: #ff7a7a;
	}
	.chat-offline {
		padding: 0.75rem 1rem;
		color: #f0c674;
		background: rgba(240, 198, 116, 0.08);
		border: 1px solid rgba(240, 198, 116, 0.3);
		border-radius: 0.4rem;
	}
	.chat-submit:disabled,
	.chat-input:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
	.chat-silence {
		padding: 0.75rem 1rem;
		color: #f0c674;
		background: rgba(240, 198, 116, 0.08);
		border: 1px solid rgba(240, 198, 116, 0.3);
		border-radius: 0.4rem;
	}
	.chat-answer {
		line-height: 1.6;
		white-space: normal;
	}
	.chat-citation-chip {
		display: inline;
		padding: 0 0.25rem;
		margin: 0 0.1rem;
		font-size: 0.8em;
		font-family: monospace;
		color: #7ab7ff;
		background: rgba(122, 183, 255, 0.12);
		border: 1px solid rgba(122, 183, 255, 0.3);
		border-radius: 0.3rem;
		cursor: pointer;
		vertical-align: baseline;
	}
	.chat-citation-chip:hover {
		background: rgba(122, 183, 255, 0.22);
	}
</style>
