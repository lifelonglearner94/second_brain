<script lang="ts">
	import { onMount } from 'svelte';
	import { adminLogs } from '$lib/admin/logs';

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

<main>
	<header>
		<h1>Admin — logs</h1>
		<p class="tagline" data-testid="admin-logs-bounded">
			Showing <code data-testid="admin-logs-count">{adminLogs.count}</code> of
			<code data-testid="admin-logs-capacity">{adminLogs.capacity}</code> recent entries.
		</p>
	</header>

	<div class="controls">
		<label class="search">
			<span class="sr-only">Search logs</span>
			<input
				type="search"
				data-testid="admin-logs-search"
				placeholder="Search message, target, fields…"
				bind:value={adminLogs.query}
			/>
		</label>
		<div class="levels" data-testid="admin-logs-filters" role="group" aria-label="Filter by level">
			<button
				type="button"
				data-testid="admin-logs-filter-all"
				class:active={adminLogs.levelFilter === 'all'}
				onclick={() => (adminLogs.levelFilter = 'all')}
			>
				All
			</button>
			{#each adminLogs.levels as level (level)}
				<button
					type="button"
					data-testid={`admin-logs-filter-${level}`}
					class:active={adminLogs.levelFilter === level}
					onclick={() => (adminLogs.levelFilter = level)}
				>
					{level}
				</button>
			{/each}
		</div>
		<button
			type="button"
			data-testid="admin-logs-refresh"
			onclick={onRefresh}
			disabled={adminLogs.status === 'loading'}
		>
			{adminLogs.status === 'loading' ? 'Refreshing…' : 'Refresh'}
		</button>
	</div>

	{#if adminLogs.status === 'loading' && adminLogs.logs.length === 0}
		<p data-testid="admin-logs-loading" class="state">Loading logs…</p>
	{:else if adminLogs.status === 'error'}
		<p data-testid="admin-logs-error" class="state error">{adminLogs.error}</p>
	{:else if adminLogs.filtered.length === 0}
		<p data-testid="admin-logs-empty" class="state">No log entries match.</p>
	{:else}
		<ol class="logs" data-testid="admin-logs-list">
			{#each adminLogs.filtered as entry (entry.timestamp + entry.message)}
				<li class="log" data-testid="admin-log-row" data-level={entry.level}>
					<div class="log-head">
						<span class="level" data-level={entry.level}>{entry.level}</span>
						<time class="ts">{fmt(entry.timestamp)}</time>
						<span class="target">{entry.target}</span>
					</div>
					<p class="message" data-testid="admin-log-message">{entry.message}</p>
					{#if fmtFields(entry.fields)}
						<pre class="fields" data-testid="admin-log-fields">{fmtFields(entry.fields)}</pre>
					{/if}
				</li>
			{/each}
		</ol>
	{/if}

	<p><a href="/app" data-testid="admin-logs-back">Back to the app</a></p>
</main>

<style>
	main {
		max-inline-size: 48rem;
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
	.sr-only {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}
	.controls {
		display: grid;
		gap: 0.75rem;
		margin: 0 0 1.5rem;
	}
	.search input {
		width: 100%;
		padding: 0.6rem 0.75rem;
		font-size: 1rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #1a1f2b;
		color: #e6e8ec;
		box-sizing: border-box;
	}
	.levels {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
	}
	.levels button,
	.controls > button {
		padding: 0.4rem 0.65rem;
		font-size: 0.85rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.4rem;
		background: #1a1f2b;
		color: #9aa3b2;
		cursor: pointer;
	}
	.levels button.active {
		border-color: #7ab7ff;
		color: #7ab7ff;
	}
	.controls > button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	.state {
		color: #9aa3b2;
	}
	.error {
		color: #ff7a7a;
	}
	.logs {
		list-style: none;
		padding: 0;
		margin: 0 0 1.5rem;
		display: grid;
		gap: 0.5rem;
	}
	.log {
		padding: 0.6rem 0.75rem;
		border: 1px solid #1f242e;
		border-radius: 0.5rem;
		background: #11141b;
	}
	.log-head {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: baseline;
		font-size: 0.8rem;
	}
	.level {
		font-family: monospace;
		font-weight: 600;
	}
	.level[data-level='ERROR'] {
		color: #ff7a7a;
	}
	.level[data-level='WARN'] {
		color: #ffb077;
	}
	.level[data-level='INFO'] {
		color: #7ab7ff;
	}
	.level[data-level='DEBUG'],
	.level[data-level='TRACE'] {
		color: #9aa3b2;
	}
	.ts {
		color: #6b7280;
		font-family: monospace;
	}
	.target {
		color: #8b93a3;
		font-family: monospace;
	}
	.message {
		margin: 0.3rem 0 0;
	}
	.fields {
		margin: 0.4rem 0 0;
		padding: 0.5rem;
		font-family: monospace;
		font-size: 0.8rem;
		color: #c4cbd6;
		background: #0b0d12;
		border-radius: 0.4rem;
		white-space: pre-wrap;
		overflow-wrap: anywhere;
	}
	a {
		color: #7ab7ff;
	}
</style>
