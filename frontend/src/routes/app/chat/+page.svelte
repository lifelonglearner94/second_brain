<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import ChatSurface from '$lib/chat/ChatSurface.svelte';
	import { OnlineStore } from '$lib/state/online.svelte';

	const onlineStore = new OnlineStore();

	onMount(() => onlineStore.init());
</script>

<main data-testid="chat-page">
	<header class="chat-header">
		<a href="/app" class="back-link" data-testid="chat-back-to-graph">← Spatial View-Graph</a>
		<h1>Chat</h1>
		{#if !onlineStore.online}
			<span class="offline-badge" data-testid="chat-page-offline-badge">Offline</span>
		{/if}
	</header>
	<ChatSurface api={apiClient} online={onlineStore.online} />
</main>

<style>
	main {
		min-block-size: 100vh;
		padding: 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
		background: #0b0d12;
		box-sizing: border-box;
	}
	.chat-header {
		display: flex;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-block-end: 1rem;
		max-inline-size: 44rem;
		margin-inline: auto;
	}
	h1 {
		margin: 0;
		font-size: clamp(1.25rem, 3vw, 1.5rem);
	}
	.back-link {
		color: #7ab7ff;
		text-decoration: none;
		font-size: 0.9rem;
	}
	.back-link:hover {
		text-decoration: underline;
	}
	.offline-badge {
		font-size: 0.8rem;
		color: #f0c674;
		background: rgba(240, 198, 116, 0.08);
		border: 1px solid rgba(240, 198, 116, 0.3);
		border-radius: 0.4rem;
		padding: 0.2rem 0.5rem;
	}
</style>
