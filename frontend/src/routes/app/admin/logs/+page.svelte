<script lang="ts">
	import { onMount } from 'svelte';
	import { adminLogs } from '$lib/state/admin-logs.svelte';

	onMount(() => {
		adminLogs.refresh();
	});

	async function onRefresh() {
		await adminLogs.refresh();
	}

	function fmt(ts: number): string {
		try {
			return new Date(ts * 1000).toLocaleString();
		} catch {
			return String(ts);
		}
	}

	function fmtFields(fields: unknown): string {
		if (fields === null || fields === undefined) return '';
		return JSON.stringify(fields, null, 2);
	}
</script>

<main class="page logs-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="admin-logs-back">
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="2"
				aria-hidden="true"
			>
				<path d="M15 18l-6-6 6-6" />
			</svg>
			Back to the app
		</a>
		<h1>Admin — logs</h1>
		<p class="tagline" data-testid="admin-logs-bounded">
			Showing
			<code class="mono" data-testid="admin-logs-count">{adminLogs.count}</code>
			of
			<code class="mono" data-testid="admin-logs-capacity"
				>{adminLogs.capacity}</code
			>
			recent entries.
		</p>
	</header>

	<div class="controls rise">
		<label class="search">
			<span class="sr-only">Search logs</span>
			<input
				class="input"
				type="search"
				data-testid="admin-logs-search"
				placeholder="Search message, target, fields…"
				bind:value={adminLogs.query}
			/>
		</label>
		<div
			class="levels"
			data-testid="admin-logs-filters"
			role="group"
			aria-label="Filter by level"
		>
			<button
				type="button"
				class="chip"
				class:active={adminLogs.levelFilter === 'all'}
				data-testid="admin-logs-filter-all"
				onclick={() => (adminLogs.levelFilter = 'all')}
			>
				All
			</button>
			{#each adminLogs.levels as level (level)}
				<button
					type="button"
					class="chip"
					class:active={adminLogs.levelFilter === level}
					data-level={level}
					data-testid={`admin-logs-filter-${level}`}
					onclick={() => (adminLogs.levelFilter = level)}
				>
					{level}
				</button>
			{/each}
		</div>
		<button
			type="button"
			class="btn btn-secondary refresh"
			data-testid="admin-logs-refresh"
			onclick={onRefresh}
			disabled={adminLogs.status === 'loading'}
		>
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="1.8"
				stroke-linecap="round"
				stroke-linejoin="round"
				aria-hidden="true"
			>
				<path d="M21 12a9 9 0 1 1-2.6-6.4M21 3v5h-5" />
			</svg>
			{adminLogs.status === 'loading' ? 'Refreshing…' : 'Refresh'}
		</button>
	</div>

	<section class="body rise">
		{#if adminLogs.status === 'loading' && adminLogs.logs.length === 0}
			<div class="state card" data-testid="admin-logs-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Loading logs…</span>
			</div>
		{:else if adminLogs.status === 'error'}
			<p class="state error pill pill-danger" data-testid="admin-logs-error">
				{adminLogs.error}
			</p>
		{:else if adminLogs.filtered.length === 0}
			<div class="state card empty" data-testid="admin-logs-empty">
				No log entries match.
			</div>
		{:else}
			<ol class="logs" data-testid="admin-logs-list">
				{#each adminLogs.filtered as entry (entry.timestamp + entry.message)}
					<li
						class="log card"
						data-testid="admin-log-row"
						data-level={entry.level}
					>
						<div class="log-head">
							<span class="badge" data-level={entry.level}>{entry.level}</span>
							<time class="ts mono">{fmt(entry.timestamp)}</time>
							<span class="target mono">{entry.target}</span>
						</div>
						<p class="message" data-testid="admin-log-message">
							{entry.message}
						</p>
						{#if fmtFields(entry.fields)}
							<pre
								class="fields mono"
								data-testid="admin-log-fields">{fmtFields(entry.fields)}</pre>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}
	</section>
</main>

<style>
	.logs-page {
		max-inline-size: 50rem;
		display: grid;
		gap: var(--space-5);
	}
	.page-head {
		display: grid;
		gap: var(--space-2);
		padding-block-end: var(--space-4);
		border-block-end: 1px solid var(--border-hairline);
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
	.page-head h1 {
		font-size: var(--fs-28);
		font-weight: 700;
	}
	.tagline {
		color: var(--fg-muted);
		font-size: var(--fs-14);
	}
	.tagline code {
		color: var(--accent);
		font-size: var(--fs-14);
		font-weight: 600;
	}

	.controls {
		display: grid;
		gap: var(--space-3);
	}
	.search .input {
		inline-size: 100%;
		min-block-size: 44px;
	}
	.levels {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}
	.chip {
		padding: 0.35rem 0.7rem;
		font-size: var(--fs-12);
		font-weight: 600;
		letter-spacing: 0.03em;
		color: var(--fg-muted);
		background: var(--bg-elevated);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-pill);
		cursor: pointer;
		transition:
			color var(--dur-1) var(--ease),
			background var(--dur-1) var(--ease),
			border-color var(--dur-1) var(--ease);
	}
	.chip:hover {
		color: var(--fg);
		border-color: var(--border-strong);
	}
	.chip.active {
		color: var(--accent);
		background: var(--accent-soft);
		border-color: var(--border-accent);
	}
	.chip[data-level='ERROR'].active {
		color: var(--danger);
		background: var(--danger-soft);
		border-color: var(--danger-border);
	}
	.chip[data-level='WARN'].active {
		color: var(--warn);
		background: var(--warn-soft);
		border-color: var(--warn-border);
	}
	.refresh {
		justify-self: start;
	}
	.refresh svg {
		inline-size: 1.05rem;
		block-size: 1.05rem;
	}
	.refresh:disabled svg {
		animation: spin 1s linear infinite;
	}
	@keyframes spin {
		to {
			transform: rotate(360deg);
		}
	}

	.state {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-4) var(--space-5);
		color: var(--fg-muted);
		font-size: var(--fs-14);
	}
	.state.empty {
		justify-content: center;
	}
	.error {
		padding: 0.6rem 0.85rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
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

	.logs {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-2);
	}
	.log {
		padding: var(--space-3) var(--space-4);
		transition: border-color var(--dur-1) var(--ease);
	}
	.log:hover {
		border-color: var(--border-strong);
	}
	.log-head {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2) var(--space-3);
		align-items: center;
		font-size: var(--fs-12);
	}
	.badge {
		padding: 0.1rem 0.5rem;
		font-family: var(--font-mono);
		font-size: var(--fs-12);
		font-weight: 600;
		letter-spacing: 0.04em;
		border-radius: var(--radius-sm);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		color: var(--fg-muted);
	}
	.badge[data-level='ERROR'] {
		color: var(--danger);
		background: var(--danger-soft);
		border-color: var(--danger-border);
	}
	.badge[data-level='WARN'] {
		color: var(--warn);
		background: var(--warn-soft);
		border-color: var(--warn-border);
	}
	.badge[data-level='INFO'] {
		color: var(--accent);
		background: var(--accent-soft);
		border-color: var(--border-accent);
	}
	.ts {
		color: var(--fg-subtle);
	}
	.target {
		color: var(--fg-muted);
	}
	.message {
		margin: var(--space-2) 0 0;
		font-size: var(--fs-14);
		color: var(--fg);
		line-height: var(--lh-body);
	}
	.fields {
		margin: var(--space-3) 0 0;
		padding: var(--space-3) var(--space-4);
		font-size: var(--fs-12);
		line-height: 1.55;
		color: var(--fg-muted);
		background: var(--bg-sunken);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
</style>
