<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { session } from '$lib/state/session.svelte';
	import { APP_TABS, isTabActive } from '$lib/state/app-tabs';

	let { children } = $props();

	$effect(() => {
		if (session.status === 'unauthenticated') {
			goto('/login', { replaceState: true });
		}
	});
</script>

{#if session.status === 'authenticated'}
	<nav class="tabs" data-testid="app-tabs" aria-label="App sections">
		{#each APP_TABS as tab (tab.href)}
			<a
				href={tab.href}
				class="tab"
				class:active={isTabActive(tab.href, page.url.pathname)}
				aria-current={isTabActive(tab.href, page.url.pathname)
					? 'page'
					: undefined}
				data-testid={`app-tab-${tab.slug}`}
			>
				{tab.label}
			</a>
		{/each}
	</nav>
	{@render children()}
{:else if session.status === 'unknown'}
	<main data-testid="auth-loading"><p>Checking session…</p></main>
{:else}
	<main data-testid="auth-redirecting"><p>Redirecting to login…</p></main>
{/if}

<style>
	.tabs {
		display: flex;
		gap: 0.25rem;
		padding: 0.4rem 0.5rem;
		background: #0b0d12;
		border-block-end: 1px solid #2a2f3a;
		overflow-x: auto;
		position: sticky;
		top: 0;
		z-index: 10;
	}
	.tab {
		flex: 0 0 auto;
		min-inline-size: 4.5rem;
		text-align: center;
		padding: 0.6rem 1rem;
		font-size: 0.95rem;
		color: #9aa3b2;
		text-decoration: none;
		border: 1px solid transparent;
		border-radius: 0.4rem;
		background: transparent;
	}
	.tab:hover {
		color: #e6e8ec;
		background: #1a1f2b;
	}
	.tab:active {
		transform: translateY(0.5px);
	}
	.tab.active {
		color: #7ab7ff;
		border-color: #2a2f3a;
		background: #1a1f2b;
	}
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
