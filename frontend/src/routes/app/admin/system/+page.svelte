<script lang="ts">
	import { onMount } from 'svelte';
	import { adminSystem } from '$lib/state/admin-system.svelte';
	import type { SystemResponse } from '$lib/api/client';

	onMount(() => {
		adminSystem.refresh();
	});

	// Local reactive alias so `{#if metrics}` narrows to non-null for the
	// template (a member access like `adminSystem.metrics` isn't narrowed
	// across reads, but a `let` binding is).
	let metrics = $derived<SystemResponse | null>(adminSystem.metrics);

	async function onRefresh() {
		await adminSystem.refresh();
	}

	function fmtBytes(bytes: number): string {
		if (!Number.isFinite(bytes) || bytes < 0) return '—';
		if (bytes < 1024) return `${bytes} B`;
		const units = ['KiB', 'MiB', 'GiB', 'TiB', 'PiB'];
		let value = bytes / 1024;
		let unit = 0;
		while (value >= 1024 && unit < units.length - 1) {
			value /= 1024;
			unit += 1;
		}
		const digits = value >= 100 ? 0 : 1;
		return `${value.toFixed(digits)} ${units[unit]}`;
	}

	function fmtPercent(pct: number): string {
		if (!Number.isFinite(pct)) return '—';
		return `${pct.toFixed(1)}%`;
	}

	// Bar widths are capped at 100% so a wildly off reading can't overflow.
	function barWidth(pct: number): number {
		return Math.max(0, Math.min(100, Number.isFinite(pct) ? pct : 0));
	}
</script>

<main class="page system-page">
	<header class="page-head rise">
		<a href="/app" class="back-link" data-testid="admin-system-back">
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
		<h1>Admin — system</h1>
		<p class="tagline" data-testid="admin-system-summary">
			Live host load — CPU, memory, disk. Refresh to sample again.
		</p>
	</header>

	<div class="controls rise">
		<button
			type="button"
			class="btn btn-secondary refresh"
			data-testid="admin-system-refresh"
			onclick={onRefresh}
			disabled={adminSystem.status === 'loading'}
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
			{adminSystem.status === 'loading' ? 'Sampling…' : 'Refresh'}
		</button>
	</div>

	<section class="body rise">
		{#if adminSystem.status === 'loading' && !metrics}
			<div class="state card" data-testid="admin-system-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Sampling host load…</span>
			</div>
		{:else if adminSystem.status === 'error'}
			<p class="state error pill pill-danger" data-testid="admin-system-error">
				{adminSystem.error}
			</p>
		{:else if !metrics}
			<div class="state card empty" data-testid="admin-system-empty">
				No metrics yet. Hit Refresh to sample.
			</div>
		{:else}
			<div class="metric card" data-testid="admin-system-cpu">
				<div class="metric-head">
					<h2>CPU</h2>
					<span class="value mono" data-testid="admin-system-cpu-percent">
						{fmtPercent(metrics.cpu.usage_percent)}
					</span>
				</div>
				<p class="subtle">
					{metrics.cpu.cores} core{metrics.cpu.cores === 1 ? '' : 's'}
				</p>
				<div class="bar" aria-hidden="true">
					<div
						class="bar-fill"
						style={`width: ${barWidth(metrics.cpu.usage_percent)}%`}
					></div>
				</div>
				{#if metrics.cpu.cores > 0}
					<ul class="cores" data-testid="admin-system-cpu-cores">
						{#each metrics.cpu.per_core as core, i (i)}
							<li class="core">
								<span class="core-label mono">#{i}</span>
								<div class="bar small" aria-hidden="true">
									<div
										class="bar-fill"
										style={`width: ${barWidth(core)}%`}
									></div>
								</div>
								<span class="core-value mono">{fmtPercent(core)}</span>
							</li>
						{/each}
					</ul>
				{/if}
			</div>

			<div class="metric card" data-testid="admin-system-memory">
				<div class="metric-head">
					<h2>Memory</h2>
					<span class="value mono" data-testid="admin-system-memory-percent">
						{fmtPercent(metrics.memory.usage_percent)}
					</span>
				</div>
				<p class="subtle">
					{fmtBytes(metrics.memory.used_bytes)} of
					{fmtBytes(metrics.memory.total_bytes)}
				</p>
				<div class="bar" aria-hidden="true">
					<div
						class="bar-fill"
						style={`width: ${barWidth(metrics.memory.usage_percent)}%`}
					></div>
				</div>
			</div>

			<div class="disk-section">
				<h2>Disk</h2>
				{#if metrics.disks.length === 0}
					<p class="state subtle">No disks visible in this environment.</p>
				{:else}
					<ul class="disks" data-testid="admin-system-disks">
						{#each metrics.disks as disk, i (i)}
							<li
								class="metric card disk"
								data-testid="admin-system-disk"
								data-brain-file={disk.mount_point ===
								metrics.brain_file_mount}
							>
								<div class="metric-head">
									<span class="mount mono">{disk.mount_point}</span>
									{#if disk.mount_point === metrics.brain_file_mount}
										<span class="badge badge-accent" data-testid="admin-system-brain-file">
											Brain File
										</span>
									{/if}
									<span class="value mono">
										{fmtPercent(disk.usage_percent)}
									</span>
								</div>
								<p class="subtle">
									{fmtBytes(disk.used_bytes)} of
									{fmtBytes(disk.total_bytes)}
								</p>
								<div class="bar" aria-hidden="true">
									<div
										class="bar-fill"
										style={`width: ${barWidth(disk.usage_percent)}%`}
									></div>
								</div>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}
	</section>
</main>

<style>
	.system-page {
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

	.controls {
		display: grid;
		gap: var(--space-3);
	}
	.refresh {
		justify-self: start;
		min-block-size: 44px;
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

	.body {
		display: grid;
		gap: var(--space-3);
	}
	.metric {
		display: grid;
		gap: var(--space-2);
		padding: var(--space-3) var(--space-4);
		transition: border-color var(--dur-1) var(--ease);
	}
	.metric:hover {
		border-color: var(--border-strong);
	}
	.metric-head {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: var(--space-2) var(--space-3);
	}
	.metric-head h2 {
		font-size: var(--fs-14);
		font-weight: 700;
		margin-inline-end: auto;
	}
	.value {
		font-size: var(--fs-18);
		font-weight: 700;
		color: var(--accent);
	}
	.subtle {
		margin: 0;
		color: var(--fg-muted);
		font-size: var(--fs-13);
	}

	.bar {
		position: relative;
		block-size: 8px;
		border-radius: var(--radius-pill);
		background: var(--bg-sunken);
		border: 1px solid var(--border-hairline);
		overflow: hidden;
	}
	.bar.small {
		block-size: 6px;
	}
	.bar-fill {
		block-size: 100%;
		border-radius: var(--radius-pill);
		background: var(--accent);
		transition: width var(--dur-2) var(--ease);
	}

	.cores {
		list-style: none;
		padding: 0;
		margin: var(--space-2) 0 0;
		display: grid;
		gap: var(--space-1);
	}
	.core {
		display: grid;
		grid-template-columns: 2rem 1fr 3.5rem;
		align-items: center;
		gap: var(--space-2);
		font-size: var(--fs-12);
	}
	.core-label {
		color: var(--fg-subtle);
	}
	.core-value {
		color: var(--fg-muted);
		text-align: end;
	}

	.disk-section {
		display: grid;
		gap: var(--space-2);
	}
	.disk-section h2 {
		font-size: var(--fs-14);
		font-weight: 700;
	}
	.disks {
		list-style: none;
		padding: 0;
		margin: 0;
		display: grid;
		gap: var(--space-2);
	}
	.disk .mount {
		font-size: var(--fs-13);
		color: var(--fg);
		margin-inline-end: auto;
		word-break: break-all;
		overflow-wrap: anywhere;
	}
	.disk[data-brain-file='true'] {
		border-color: var(--border-accent);
	}
	.badge-accent {
		color: var(--accent);
		background: var(--accent-soft);
		border: 1px solid var(--border-accent);
		border-radius: var(--radius-sm);
		padding: 0.1rem 0.5rem;
		font-family: var(--font-mono);
		font-size: var(--fs-11);
		font-weight: 600;
		letter-spacing: 0.04em;
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
</style>
