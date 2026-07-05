<script lang="ts">
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import type { GraphDelta } from '$lib/api/client';
	import { createIdb } from '$lib/state/idb';
	import { loadViewport, saveViewport } from '$lib/state/viewport';
	import { frozenGraphStatus } from '$lib/graph/frozen-graph';
	import {
		buildSpatialViewGraph,
		type GraphData,
		type GraphNode,
		type GraphLink
	} from '$lib/graph/build';
	import { detectRendererCapability, probeRendererCapability } from '$lib/graph/capability';
	import { renderSpatialViewGraph2D, type Render2DHandle } from '$lib/graph/render2d';
	import { onWindowFocus } from '$lib/graph/delta-sync';
	import { graphStore } from '$lib/state/graph.svelte';

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
	let fetchedAtLabel = $state<string | null>(null);
	let selectedNodeId = $state<string | null>(null);
	let selectedLabel = $state<string | null>(null);

	let graphContainer = $state<HTMLDivElement | null>(null);
	let fg: FgInstance | null = null;
	let handle2d: Render2DHandle | null = null;
	let rendererChoice: '3d' | '2d' = '3d';
	let online = $state(typeof navigator !== 'undefined' ? navigator.onLine : true);

	function onNodeSelect(id: string | null, label: string | null) {
		selectedNodeId = id;
		selectedLabel = label;
		persistViewport();
	}

	function persistViewport() {
		if (fg) {
			const cam = fg.cameraPosition();
			saveViewport({
				cameraX: cam.x,
				cameraY: cam.y,
				cameraZ: cam.z,
				zoom: 0,
				selectedNodeId
			});
		} else if (handle2d) {
			saveViewport({ cameraX: 0, cameraY: 0, cameraZ: 0, zoom: 0, selectedNodeId });
		}
	}

	onMount(() => {
		let destroyed = false;
		const idb = createIdb();
		const savedViewport = loadViewport();

		(async () => {
			try {
				const { source, fetchedAt } = await graphStore.loadFromNetworkOrCache(apiClient, idb);
				if (destroyed) return;
				const frozen = frozenGraphStatus(source, fetchedAt, online);
				fetchedAtLabel = frozen.label;
				const svg = buildSpatialViewGraph(graphStore.snapshot!);
				rendererChoice = detectRendererCapability(probeRendererCapability());
				if (rendererChoice === '2d') {
					await renderGraph2D(svg, frozen.status);
				} else {
					await renderGraph3D(graphStore.data, frozen.status);
				}
			} catch (e) {
				if (destroyed) return;
				const msg = e instanceof Error ? e.message : String(e);
				const frozen = frozenGraphStatus('error', null, online, msg);
				fetchedAtLabel = frozen.label;
				status = frozen.status;
			}
		})();

		const stopFocusSync = onWindowFocus(globalThis, () => {
			void reconcileOnFocus();
		});

		function handleConnectivity(): void {
			online = typeof navigator !== 'undefined' ? navigator.onLine : true;
		}
		globalThis.addEventListener('online', handleConnectivity);
		globalThis.addEventListener('offline', handleConnectivity);

		async function reconcileOnFocus(): Promise<void> {
			if (destroyed || !graphStore.snapshot) return;
			const outcome = await graphStore.syncDelta(apiClient);
			if (destroyed || !outcome.applied) return;
			if (fg && outcome.delta && hasDeltaChanges(outcome.delta)) {
				fg.graphData(graphStore.data);
			}
		}

		function hasDeltaChanges(delta: GraphDelta): boolean {
			return (
				delta.added_concepts.length > 0 ||
				delta.added_edges.length > 0 ||
				delta.deleted_concept_ids.length > 0 ||
				delta.deleted_edge_ids.length > 0 ||
				delta.retagged_edges.length > 0
			);
		}

		async function renderGraph3D(data: GraphData, finalStatus: Status) {
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
				.nodeRelSize(7)
				.nodeOpacity(0.95)
				.nodeColor((n) => (n.id === selectedNodeId ? HIGHLIGHT : n.color))
				.nodeLabel((n) => n.label)
				.linkColor((l) => l.color)
				.linkOpacity(0.4)
				.linkDirectionalParticles(2)
				.linkDirectionalParticleSpeed(0.004)
				.onNodeClick((n) => onNodeSelect(n.id, n.label))
				.onBackgroundClick(() => onNodeSelect(null, null));

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
					onNodeSelect(restored.id, restored.label);
				}
			}

			const controls = instance.controls();
			controls.addEventListener('end', persistViewport);

			status = finalStatus;
		}

		async function renderGraph2D(
			svg: ReturnType<typeof buildSpatialViewGraph>,
			finalStatus: Status
		) {
			if (destroyed || !graphContainer) return;
			handle2d = await renderSpatialViewGraph2D(graphContainer, svg, {
				selectedNodeId: savedViewport?.selectedNodeId ?? null,
				onSelectNode: (id, label) => onNodeSelect(id, label)
			});
			if (destroyed) {
				handle2d?.destroy();
				handle2d = null;
				return;
			}
			if (savedViewport?.selectedNodeId && svg.hasNode(savedViewport.selectedNodeId)) {
				const label = svg.getNodeAttribute(savedViewport.selectedNodeId, 'label') as string;
				handle2d.setSelected(savedViewport.selectedNodeId);
				onNodeSelect(savedViewport.selectedNodeId, label);
			}
			status = finalStatus;
		}

		return () => {
			destroyed = true;
			stopFocusSync();
			globalThis.removeEventListener('online', handleConnectivity);
			globalThis.removeEventListener('offline', handleConnectivity);
			try {
				fg?._destructor();
			} catch {
				/* noop */
			}
			fg = null;
			try {
				handle2d?.destroy();
			} catch {
				/* noop */
			}
			handle2d = null;
		};
	});
</script>

<main>
	<section class="graph-section" data-testid="graph-view" aria-live="polite">
		<div class="graph-canvas" bind:this={graphContainer}></div>

		{#if status === 'loading'}
			<p class="status" data-testid="graph-loading">Loading the Spatial View-Graph…</p>
		{:else if status === 'offline'}
			<p class="status stale" data-testid="graph-offline">
				{fetchedAtLabel}
			</p>
		{:else if status === 'error'}
			<p class="status error" data-testid="graph-error">
				{fetchedAtLabel}
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
	.graph-section {
		position: relative;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		overflow: hidden;
		block-size: calc(100vh - 6rem);
		min-block-size: 20rem;
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
