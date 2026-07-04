<script lang="ts">
	import { goto } from '$app/navigation';
	import { session } from '$lib/state/session.svelte';

	let { children } = $props();

	$effect(() => {
		if (session.status === 'unauthenticated') {
			goto('/login', { replaceState: true });
		}
	});
</script>

{#if session.status === 'authenticated'}
	{@render children()}
{:else if session.status === 'unknown'}
	<main data-testid="auth-loading"><p>Checking session…</p></main>
{:else}
	<main data-testid="auth-redirecting"><p>Redirecting to login…</p></main>
{/if}

<style>
	main {
		max-inline-size: 40rem;
		margin-inline: auto;
		padding: 2rem 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #9aa3b2;
		background: #0b0d12;
		min-block-size: 100vh;
		box-sizing: border-box;
	}
</style>
