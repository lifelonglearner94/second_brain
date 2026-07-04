<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { apiClient } from '$lib/api';
	import { session } from '$lib/auth/session';
	import { createIdb } from '$lib/state/idb';
	import { loadViewport, saveViewport } from '$lib/state/viewport';
	import { loadSpatialViewGraph } from '$lib/graph/load';
	import { buildGraphData, type GraphData, type GraphNode, type GraphLink } from '$lib/graph/build';

	const HIGHLIGHT = '#ffffff';

	type Status = 'loading' | 'ready' | 'offline' | 'error';

	type Coords = { x: number; y: number; z: number };

	type FgInstance = {
		width(n: number): FgInstance;
		height(n: number): FgInstance;
		backgroundColor(c: string): FgInstance;
		graphData(d: GraphData): FgInstance;
		nodeRelSize(n: number): FgInstance;
		nodeOpacity(n: number): FgInstance;
		nodeColor(fn: (n: GraphNode) => string): FgInstance;
		nodeLabel(fn: (n: GraphNode) => string): FgInstance;
		linkColor(fn: (l: GraphLink) => string): FgInstance;
		linkOpacity(n: number): FgInstance;
		linkDirectionalParticles(n: number): FgInstance;
		linkDirectionalParticleSpeed(n: number): FgInstance;
		onNodeClick(cb: (n: GraphNode) => void): FgInstance;
		onBackgroundClick(cb: () => void): FgInstance;
		postProcessingComposer(): { addPass(p: unknown): void };
		cameraPosition(): Coords;
		cameraPosition(pos: Partial<Coords>): FgInstance;
		controls(): { addEventListener(name: 'end', handler: () => void): void };
		_destructor(): void;
	};

	let status = $state<Status>('loading');
	let errorMessage = $state<string | null>(null);
	let fetchedAtLabel = $state<string | null>(null);
	let selectedNodeId = $state<string | null>(null);
	let selectedLabel = $state<string | null>(null);

	let busy = $state(false);
	let logoutError = $state<string | null>(null);

	let graphContainer = $state<HTMLDivElement | null>(null);
	let fg: FgInstance | null = null;
	async function onLogout() {
		busy = true;
		logoutError = null;
		try {
			await apiClient.logout();
			session.clear();
			await goto('/login', { replaceState: true });
		} catch (e) {
			logoutError = e instanceof Error ? e.message : String(e);
		} finally {
			busy = false;
		}
	}

	function selectNode(id: string | null, label: string | null) {
		selectedNodeId = id;
		selectedLabel = label;
		persistViewport();
	}

	function persistViewport() {
		if (!fg) return;
		const cam = fg.cameraPosition();
		saveViewport({
			cameraX: cam.x,
			cameraY: cam.y,
			cameraZ: cam.z,
			zoom: 0,
			selectedNodeId
		});
	}

	onMount(() => {
		let destroyed = false;
		const idb = createIdb();
		const savedViewport = loadViewport();

		(async () => {
			try {
				const loaded = await loadSpatialViewGraph(apiClient, idb);
				if (destroyed) return;
				fetchedAtLabel = loaded.source === 'cache' ? loaded.snapshot.fetchedAt : null;
				const data = buildGraphData(loaded.snapshot);
				await renderGraph(data, loaded.source === 'cache' ? 'offline' : 'ready');
			} catch (e) {
				if (destroyed) return;
				errorMessage = e instanceof Error ? e.message : String(e);
				status = 'error';
			}
		})();

		async function renderGraph(data: GraphData, finalStatus: Status) {
			const [{ default: ForceGraph3D }, { Vector2 }, { UnrealBloomPass }] = await Promise.all([
				import('3d-force-graph'),
				import('three'),
				import('three/examples/jsm/postprocessing/UnrealBloomPass.js')
			]);
			if (destroyed || !graphContainer) return;

			const instance = new ForceGraph3D(graphContainer, {
				controlType: 'orbit'
			}) as unknown as FgInstance;
			fg = instance;

			instance
				.width(graphContainer.clientWidth)
				.height(graphContainer.clientHeight)
				.backgroundColor('#0b0d12')
				.graphData(data)
				.nodeRelSize(2.5)
				.nodeOpacity(0.95)
				.nodeColor((n) => (n.id === selectedNodeId ? HIGHLIGHT : n.color))
				.nodeLabel((n) => n.label)
				.linkColor((l) => l.color)
				.linkOpacity(0.4)
				.linkDirectionalParticles(2)
				.linkDirectionalParticleSpeed(0.004)
				.onNodeClick((n) => selectNode(n.id, n.label))
				.onBackgroundClick(() => selectNode(null, null));

			instance.postProcessingComposer().addPass(
				new UnrealBloomPass(
					new Vector2(graphContainer.clientWidth, graphContainer.clientHeight),
					1.4,
					0.6,
					0.1
				)
			);

			if (savedViewport) {
				instance.cameraPosition({
					x: savedViewport.cameraX,
					y: savedViewport.cameraY,
					z: savedViewport.cameraZ
				});
				const restored = data.nodes.find((n) => n.id === savedViewport!.selectedNodeId);
				if (restored) {
					selectNode(restored.id, restored.label);
				}
			}

			const controls = instance.controls();
			controls.addEventListener('end', persistViewport);

			status = finalStatus;
		}

		return () => {
			destroyed = true;
			try {
				fg?._destructor();
			} catch {
				/* noop */
			}
			fg = null;
		};
	});
</script>

<main>
	<header>
		<h1>Second Brain</h1>
		<p class="tagline">Signed in as <code data-testid="user-id">{session.userId}</code></p>
		<button
			type="button"
			data-testid="logout-button"
			onclick={onLogout}
			disabled={busy}
		>
			{busy ? 'Signing out…' : 'Sign out'}
		</button>
		{#if logoutError}
			<p data-testid="logout-error" class="error">{logoutError}</p>
		{/if}
	</header>

	<section class="graph-section" data-testid="graph-view" aria-live="polite">
		<div class="graph-canvas" bind:this={graphContainer}></div>

		{#if status === 'loading'}
			<p class="status" data-testid="graph-loading">Loading the Spatial View-Graph…</p>
		{:else if status === 'offline'}
			<p class="status stale" data-testid="graph-offline">
				Offline — showing graph as of {fetchedAtLabel} (Frozen Graph).
			</p>
		{:else if status === 'error'}
			<p class="status error" data-testid="graph-error">
				Could not load the graph: {errorMessage}
			</p>
		{:else}
			<p class="status" data-testid="graph-ready">Spatial View-Graph ready.</p>
		{/if}

		<aside class="selected" data-testid="selected-node-panel">
			{#if selectedLabel}
				<span data-testid="selected-node-label">{selectedLabel}</span>
			{:else}
				<span class="muted" data-testid="selected-node-label">No concept selected.</span>
			{/if}
		</aside>
	</section>
</main>

<style>
	main {
		margin-inline: auto;
		padding: 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
		background: #0b0d12;
		min-block-size: 100vh;
		box-sizing: border-box;
	}
	header {
		display: flex;
		align-items: center;
		gap: 1rem;
		flex-wrap: wrap;
		margin-block-end: 1rem;
	}
	h1 {
		margin: 0;
		font-size: clamp(1.25rem, 3vw, 1.5rem);
	}
	.tagline {
		margin: 0;
		color: #9aa3b2;
	}
	code {
		font-family: monospace;
		color: #7ab7ff;
	}
	button {
		margin-inline-start: auto;
		padding: 0.5rem 1rem;
		font-size: 0.95rem;
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
	.graph-section {
		position: relative;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		overflow: hidden;
		block-size: calc(100vh - 7rem);
		min-block-size: 24rem;
		background: #0b0d12;
	}
	.graph-canvas {
		position: absolute;
		inset: 0;
	}
	.status {
		position: absolute;
		top: 0.75rem;
		left: 0.75rem;
		margin: 0;
		padding: 0.4rem 0.6rem;
		font-size: 0.85rem;
		color: #9aa3b2;
		background: rgba(11, 13, 18, 0.7);
		border-radius: 0.4rem;
		pointer-events: none;
	}
	.status.stale {
		color: #f0c674;
	}
	.status.error {
		color: #ff7a7a;
	}
	.selected {
		position: absolute;
		bottom: 0.75rem;
		left: 0.75rem;
		margin: 0;
		padding: 0.4rem 0.6rem;
		font-size: 0.95rem;
		color: #e6e8ec;
		background: rgba(11, 13, 18, 0.7);
		border-radius: 0.4rem;
		pointer-events: none;
	}
	.selected .muted {
		color: #9aa3b2;
	}
	.error {
		color: #ff7a7a;
	}
</style>
