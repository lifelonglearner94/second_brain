<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient, type Health } from '$lib/api';
	import { session } from '$lib/state/session.svelte';

	let health: Health | null = $state(null);
	let error: string | null = $state(null);
	let loading = $state(true);

	onMount(async () => {
		try {
			health = await apiClient.getHealth();
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			loading = false;
		}
	});
</script>

<main>
	<header>
		<h1>Second Brain</h1>
		<p class="tagline">Voice capture, a 3D knowledge graph, and grounded chat.</p>
	</header>

	<nav data-testid="auth-nav" aria-live="polite">
		{#if session.status === 'authenticated'}
			<a href="/app" data-testid="goto-app">Open the app</a>
		{:else if session.status === 'unauthenticated'}
			<a href="/login" data-testid="goto-login">Sign in with a passkey</a>
		{/if}
	</nav>

	<section data-testid="health" aria-live="polite">
		<h2>Backend</h2>
		{#if loading}
			<p data-testid="health-loading">Checking backend…</p>
		{:else if error}
			<p data-testid="health-error">Backend unreachable: {error}</p>
		{:else if health}
			<p data-testid="health-ok">
				Backend {health.ok ? 'healthy' : 'unhealthy'}
			</p>
			<ul>
				<li>db: {String(health.db)}</li>
				<li>sqlite_vec: {String(health.sqlite_vec)}</li>
			</ul>
		{/if}
	</section>
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
		font-size: clamp(1.75rem, 5vw, 2.5rem);
	}
	.tagline {
		margin: 0 0 2rem;
		color: #9aa3b2;
	}
	h2 {
		font-size: 1rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: #9aa3b2;
		margin: 0 0 0.5rem;
	}
	nav {
		margin: 0 0 2rem;
	}
	nav a {
		color: #7ab7ff;
		text-decoration: underline;
		font-size: 1.05rem;
	}
	ul {
		list-style: none;
		padding: 0;
		margin: 0.5rem 0 0;
		font-family: monospace;
	}
</style>
