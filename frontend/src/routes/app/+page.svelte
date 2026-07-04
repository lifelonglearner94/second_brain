<script lang="ts">
	import { goto } from '$app/navigation';
	import { apiClient } from '$lib/api';
	import { session } from '$lib/auth/session';

	let busy = $state(false);
	let error = $state<string | null>(null);

	async function onLogout() {
		busy = true;
		error = null;
		try {
			await apiClient.logout();
			session.clear();
			await goto('/login', { replaceState: true });
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}
</script>

<main>
	<header>
		<h1>Second Brain</h1>
		<p class="tagline">Signed in as <code data-testid="user-id">{session.userId}</code></p>
	</header>

	<p class="placeholder" data-testid="app-placeholder">
		The graph and chat surface land in a later slice.
	</p>

	<button type="button" data-testid="logout-button" onclick={onLogout} disabled={busy}>
		{busy ? 'Signing out…' : 'Sign out'}
	</button>

	{#if error}
		<p data-testid="logout-error" class="error">{error}</p>
	{/if}
</main>

<style>
	main {
		max-inline-size: 40rem;
		margin-inline: auto;
		padding: 2rem 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
		background: #0b0d12;
		min-block-size: 100vh;
		box-sizing: border-box;
	}
	h1 {
		margin: 0 0 0.25rem;
		font-size: clamp(1.5rem, 4vw, 2rem);
	}
	.tagline {
		margin: 0 0 1.5rem;
		color: #9aa3b2;
	}
	code {
		font-family: monospace;
		color: #7ab7ff;
	}
	.placeholder {
		color: #9aa3b2;
		margin: 0 0 1.5rem;
	}
	button {
		padding: 0.75rem 1rem;
		font-size: 1rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	.error {
		color: #ff7a7a;
	}
</style>
