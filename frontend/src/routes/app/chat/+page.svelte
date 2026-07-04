<script lang="ts">
	import { apiClient } from '$lib/api';
	import ChatSurface from '$lib/chat/ChatSurface.svelte';
</script>

<main data-testid="chat-page">
	<header class="chat-header">
		<a href="/app" class="back-link" data-testid="chat-back-to-graph">← Spatial View-Graph</a>
		<h1>Chat</h1>
	</header>
	<ChatSurface api={apiClient} />
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
</style>
