<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import ChatSurface from '$lib/chat/ChatSurface.svelte';
	import { OnlineStore } from '$lib/state/online.svelte';

	const onlineStore = new OnlineStore();

	onMount(() => onlineStore.init());
</script>

<main class="chat-page" data-testid="chat-page">
	<header class="chat-header">
		<a href="/app" class="back-link" data-testid="chat-back-to-graph">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				aria-hidden="true"
			>
				<path d="M15 18l-6-6 6-6" />
			</svg>
			<span>Spatial View-Graph</span>
		</a>
		<h1>Chat</h1>
		{#if !onlineStore.online}
			<span
				class="pill pill-warn offline-badge"
				data-testid="chat-page-offline-badge">Offline</span
			>
		{/if}
	</header>
	<ChatSurface api={apiClient} online={onlineStore.online} />
</main>

<style>
	.chat-page {
		min-block-size: 100dvh;
		padding: var(--space-4);
		display: grid;
		gap: var(--space-4);
	}
	.chat-header {
		max-inline-size: 46rem;
		margin-inline: auto;
		width: 100%;
		display: flex;
		align-items: center;
		gap: var(--space-3);
		flex-wrap: wrap;
	}
	.back-link {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		color: var(--fg-muted);
		font-size: var(--fs-13);
	}
	.back-link:hover {
		color: var(--accent);
	}
	.back-link svg {
		inline-size: 1rem;
		block-size: 1rem;
	}
	.chat-header h1 {
		font-size: var(--fs-22);
		font-weight: 600;
	}
	.offline-badge {
		text-transform: none;
		letter-spacing: normal;
		font-weight: 500;
	}
</style>
