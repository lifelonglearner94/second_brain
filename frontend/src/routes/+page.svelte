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

<main class="hero">
	<div class="hero-glow" aria-hidden="true"></div>

	<section class="hero-card rise">
		<header class="brand">
			<div class="brand-mark" aria-hidden="true">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.6"
				>
					<circle cx="6" cy="7" r="2.2" />
					<circle cx="18" cy="7" r="2.2" />
					<circle cx="12" cy="17" r="2.6" />
					<path d="M7.6 8.4 10.6 15" />
					<path d="M16.4 8.4 13.4 15" />
					<path d="M8.2 7 15.8 7" />
				</svg>
			</div>
			<div>
				<p class="eyebrow">A second brain</p>
				<h1>Second&nbsp;Brain</h1>
			</div>
		</header>

		<p class="lede">
			Voice capture, a 3D knowledge graph, and grounded chat - a single canvas
			for the thoughts you don't want to lose.
		</p>

		<nav class="cta" data-testid="auth-nav" aria-live="polite">
			{#if session.status === 'authenticated'}
				<a
					href="/app"
					class="btn btn-primary cta-primary"
					data-testid="goto-app"
				>
					Open the app
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						aria-hidden="true"
					>
						<path d="M5 12h14M13 6l6 6-6 6" />
					</svg>
				</a>
			{:else if session.status === 'unauthenticated'}
				<a
					href="/login"
					class="btn btn-primary cta-primary"
					data-testid="goto-login"
				>
					Sign in with a passkey
					<svg
						viewBox="0 0 24 24"
						fill="none"
						stroke="currentColor"
						stroke-width="2"
						aria-hidden="true"
					>
						<path d="M5 12h14M13 6l6 6-6 6" />
					</svg>
				</a>
			{:else}
				<span class="cta-pending" aria-busy="true">Checking session…</span>
			{/if}
		</nav>

		<section class="status card" data-testid="health" aria-live="polite">
			<div class="status-head">
				<span class="eyebrow">Backend</span>
				{#if !loading && !error && health}
					<span
						class="pill"
						class:pill-success={health.ok}
						class:pill-danger={!health.ok}
					>
						{health.ok ? 'healthy' : 'unhealthy'}
					</span>
				{/if}
			</div>

			{#if loading}
				<p class="status-line" data-testid="health-loading">
					<span class="dot-pulse" aria-hidden="true"></span>
					Checking backend…
				</p>
			{:else if error}
				<p class="status-line error" data-testid="health-error">
					Backend unreachable: {error}
				</p>
			{:else if health}
				<p class="status-line ok" data-testid="health-ok">
					Backend {health.ok ? 'healthy' : 'unhealthy'}
				</p>
				<dl class="status-grid">
					<div>
						<dt>db</dt>
						<dd class="mono">{String(health.db)}</dd>
					</div>
					<div>
						<dt>sqlite_vec</dt>
						<dd class="mono">{String(health.sqlite_vec)}</dd>
					</div>
				</dl>
			{/if}
		</section>
	</section>
</main>

<style>
	.hero {
		position: relative;
		min-block-size: 100dvh;
		display: grid;
		place-items: center;
		padding: var(--space-8) var(--space-4);
		overflow: hidden;
	}
	.hero-glow {
		position: absolute;
		inset: -20%;
		background:
			radial-gradient(
				40rem 28rem at 18% 22%,
				rgba(122, 183, 255, 0.14),
				transparent 65%
			),
			radial-gradient(
				36rem 26rem at 86% 78%,
				rgba(183, 167, 255, 0.1),
				transparent 68%
			);
		filter: blur(8px);
		pointer-events: none;
	}
	.hero-card {
		position: relative;
		z-index: 1;
		max-inline-size: 34rem;
		width: 100%;
		display: grid;
		gap: var(--space-6);
		padding: var(--space-10) var(--space-8);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-xl);
		box-shadow: var(--shadow-2);
		backdrop-filter: blur(14px) saturate(120%);
		-webkit-backdrop-filter: blur(14px) saturate(120%);
	}
	.brand {
		display: flex;
		align-items: center;
		gap: var(--space-4);
	}
	.brand-mark {
		display: grid;
		place-items: center;
		inline-size: 3rem;
		block-size: 3rem;
		flex: 0 0 auto;
		color: var(--accent);
		background: var(--accent-soft);
		border: 1px solid var(--border-accent);
		border-radius: var(--radius-lg);
		box-shadow: 0 0 28px -8px var(--accent-glow);
	}
	.brand-mark svg {
		inline-size: 1.75rem;
		block-size: 1.75rem;
	}
	.brand h1 {
		font-size: var(--fs-28);
		font-weight: 700;
		letter-spacing: -0.02em;
	}
	.lede {
		font-size: var(--fs-18);
		line-height: var(--lh-read);
		color: var(--fg-muted);
		max-inline-size: 32rem;
	}
	.cta {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.cta-primary {
		min-block-size: 48px;
		padding: 0.7rem 1.2rem;
		font-size: var(--fs-16);
	}
	.cta-primary svg {
		inline-size: 1.15rem;
		block-size: 1.15rem;
		transition: transform var(--dur-2) var(--ease);
	}
	.cta-primary:hover svg {
		transform: translateX(3px);
	}
	.cta-pending {
		color: var(--fg-subtle);
		font-size: var(--fs-14);
	}

	.status {
		padding: var(--space-4) var(--space-5);
		display: grid;
		gap: var(--space-3);
	}
	.status-head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
	}
	.status-line {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		font-size: var(--fs-14);
		color: var(--fg-muted);
	}
	.status-line.ok {
		color: var(--fg);
	}
	.status-line.error {
		color: var(--danger);
	}
	.dot-pulse {
		inline-size: 7px;
		block-size: 7px;
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
	.status-grid {
		display: grid;
		grid-template-columns: repeat(2, 1fr);
		gap: var(--space-3);
		margin: 0;
		padding-top: var(--space-3);
		border-top: 1px solid var(--border-hairline);
	}
	.status-grid > div {
		display: grid;
		gap: 0.15rem;
	}
	.status-grid dt {
		font-size: var(--fs-12);
		text-transform: uppercase;
		letter-spacing: var(--tracking-label);
		color: var(--fg-subtle);
	}
	.status-grid dd {
		margin: 0;
		color: var(--accent);
	}

	@media (max-width: 480px) {
		.hero-card {
			padding: var(--space-8) var(--space-5);
		}
		.brand h1 {
			font-size: var(--fs-22);
		}
		.lede {
			font-size: var(--fs-16);
		}
	}
</style>
