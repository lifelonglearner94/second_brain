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

	const ICONS: Record<string, string> = {
		capture:
			'M12 14a3 3 0 0 0 3-3V6a3 3 0 1 0-6 0v5a3 3 0 0 0 3 3Zm7-3a7 7 0 0 1-14 0M12 18v3',
		graph:
			'M6 7a2.2 2.2 0 1 0 0-.01M18 7a2.2 2.2 0 1 0 0-.01M12 17a2.6 2.6 0 1 0 0-.01M7.6 8.4 10.6 15M16.4 8.4 13.4 15M8.2 7 15.8 7',
		chat: 'M4 5h16v11H8l-4 4V5ZM8 9h8M8 12h5'
	};
</script>

{#if session.status === 'authenticated'}
	<div class="shell">
		<nav class="tabs" data-testid="app-tabs" aria-label="App sections">
			<div class="tabs-inner">
				{#each APP_TABS as tab (tab.href)}
					{@const active = isTabActive(tab.href, page.url.pathname)}
					<a
						href={tab.href}
						class="tab"
						class:active
						aria-current={active ? 'page' : undefined}
						data-testid={`app-tab-${tab.slug}`}
					>
						<svg
							viewBox="0 0 24 24"
							fill="none"
							stroke="currentColor"
							stroke-width="1.7"
							stroke-linecap="round"
							stroke-linejoin="round"
							aria-hidden="true"
						>
							<path d={ICONS[tab.slug]} />
						</svg>
						<span class="tab-label">{tab.label}</span>
						<span class="tab-indicator" aria-hidden="true"></span>
					</a>
				{/each}
			</div>
		</nav>
		<div class="shell-main">
			{@render children()}
		</div>
	</div>
{:else if session.status === 'unknown'}
	<main class="gate" data-testid="auth-loading">
		<span class="dot-pulse" aria-hidden="true"></span>
		<p>Checking session…</p>
	</main>
{:else}
	<main class="gate" data-testid="auth-redirecting">
		<p>Redirecting to login…</p>
	</main>
{/if}

<style>
	.shell {
		min-block-size: 100dvh;
		display: flex;
		flex-direction: column;
	}
	.tabs {
		position: sticky;
		top: 0;
		z-index: var(--z-sticky);
		background: rgba(8, 9, 13, 0.72);
		border-block-end: 1px solid var(--border-hairline);
		backdrop-filter: blur(14px) saturate(130%);
		-webkit-backdrop-filter: blur(14px) saturate(130%);
	}
	.tabs-inner {
		max-inline-size: 48rem;
		margin-inline: auto;
		display: flex;
		gap: var(--space-1);
		padding: var(--space-2) var(--space-3);
		overflow-x: auto;
		scrollbar-width: none;
	}
	.tabs-inner::-webkit-scrollbar {
		display: none;
	}
	.tab {
		position: relative;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		flex: 0 0 auto;
		min-block-size: 44px;
		padding: 0.55rem 0.9rem;
		font-size: var(--fs-14);
		font-weight: 500;
		color: var(--fg-muted);
		border-radius: var(--radius-md);
		transition:
			color var(--dur-1) var(--ease),
			background var(--dur-1) var(--ease);
		-webkit-tap-highlight-color: transparent;
	}
	.tab svg {
		inline-size: 1.15rem;
		block-size: 1.15rem;
		opacity: 0.85;
		flex: 0 0 auto;
	}
	.tab:hover {
		color: var(--fg);
		background: var(--surface-glass);
	}
	.tab.active {
		color: var(--accent-strong);
	}
	.tab.active svg {
		opacity: 1;
	}
	.tab-indicator {
		position: absolute;
		inset-block-end: 2px;
		inset-inline: 0.9rem;
		block-size: 2px;
		background: var(--accent);
		border-radius: var(--radius-pill);
		box-shadow: 0 0 12px -2px var(--accent-glow);
		transform: scaleX(0);
		transform-origin: center;
		opacity: 0;
		transition:
			transform var(--dur-2) var(--ease),
			opacity var(--dur-2) var(--ease);
	}
	.tab.active .tab-indicator {
		transform: scaleX(1);
		opacity: 1;
	}
	.shell-main {
		flex: 1 1 auto;
		min-block-size: 0;
	}

	.gate {
		min-block-size: 100dvh;
		display: grid;
		place-items: center;
		gap: var(--space-3);
		color: var(--fg-muted);
		font-size: var(--fs-14);
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

	@media (max-width: 480px) {
		.tab-label {
			display: none;
		}
		.tab {
			min-inline-size: 56px;
			justify-content: center;
			padding: 0.55rem 0.7rem;
		}
		.tab-indicator {
			inset-inline: 0.7rem;
		}
	}
</style>
