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
	import {
		detectRendererCapability,
		probeRendererCapability
	} from '$lib/graph/capability';
	import {
		renderSpatialViewGraph2D,
		type Render2DHandle
	} from '$lib/graph/render2d';
	import { onWindowFocus } from '$lib/graph/delta-sync';
	import { graphStore } from '$lib/state/graph.svelte';
	import { housekeeping } from '$lib/state/housekeeping.svelte';

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
	let online = $state(
		typeof navigator !== 'undefined' ? navigator.onLine : true
	);

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
			saveViewport({
				cameraX: 0,
				cameraY: 0,
				cameraZ: 0,
				zoom: 0,
				selectedNodeId
			});
		}
	}

	onMount(() => {
		let destroyed = false;
		const idb = createIdb();
		const savedViewport = loadViewport();

		(async () => {
			try {
				const { source, fetchedAt } = await graphStore.loadFromNetworkOrCache(
					apiClient,
					idb
				);
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

		void housekeeping.load();

		const stopFocusSync = onWindowFocus(globalThis, () => {
			void reconcileOnFocus();
		});

		function handleConnectivity(): void {
			online = typeof navigator !== 'undefined' ? navigator.onLine : true;
		}
		globalThis.addEventListener('online', handleConnectivity);
		globalThis.addEventListener('offline', handleConnectivity);

		async function reconcileOnFocus(): Promise<void> {
			if (destroyed) return;
			void housekeeping.load();
			if (!graphStore.snapshot) return;
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
			const [{ default: ForceGraph3D }, { Vector2 }, { UnrealBloomPass }] =
				await Promise.all([
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

			instance
				.postProcessingComposer()
				.addPass(
					new UnrealBloomPass(
						new Vector2(
							graphContainer.clientWidth,
							graphContainer.clientHeight
						),
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
				const restored = data.nodes.find(
					(n) => n.id === savedViewport!.selectedNodeId
				);
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
			if (
				savedViewport?.selectedNodeId &&
				svg.hasNode(savedViewport.selectedNodeId)
			) {
				const label = svg.getNodeAttribute(
					savedViewport.selectedNodeId,
					'label'
				) as string;
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

<main class="graph-page">
	<section class="graph-section" data-testid="graph-view" aria-live="polite">
		<div class="graph-canvas" bind:this={graphContainer}></div>
		<div class="graph-vignette" aria-hidden="true"></div>

		{#if status === 'loading'}
			<div class="overlay overlay-status glass" data-testid="graph-loading">
				<span class="dot-pulse" aria-hidden="true"></span>
				<span>Loading the Spatial View-Graph…</span>
			</div>
		{:else if status === 'offline'}
			<div
				class="overlay overlay-status glass warn"
				data-testid="graph-offline"
			>
				<span class="dot" aria-hidden="true"></span>
				<span>{fetchedAtLabel}</span>
			</div>
		{:else if status === 'error'}
			<div
				class="overlay overlay-status glass danger"
				data-testid="graph-error"
			>
				<span class="dot" aria-hidden="true"></span>
				<span>{fetchedAtLabel}</span>
			</div>
		{:else}
			<div class="overlay overlay-status glass" data-testid="graph-ready">
				<span class="dot ok" aria-hidden="true"></span>
				<span>Spatial View-Graph ready.</span>
			</div>
		{/if}

		<aside
			class="overlay overlay-selected glass"
			data-testid="selected-node-panel"
		>
			{#if selectedLabel}
				<span class="selected-dot" aria-hidden="true"></span>
				<span class="selected-label" data-testid="selected-node-label"
					>{selectedLabel}</span
				>
			{:else}
				<span class="selected-label muted" data-testid="selected-node-label"
					>No concept selected.</span
				>
			{/if}
		</aside>

		{#if housekeeping.status === 'loaded' && housekeeping.items.length > 0}
			<aside
				class="overlay overlay-housekeeping glass"
				data-testid="housekeeping-banner"
				aria-label="Housekeeping queue"
			>
				<span class="hk-dot" aria-hidden="true"></span>
				<span class="hk-count">
					{housekeeping.items.length} concept{housekeeping.items.length === 1
						? ''
						: 's'} to resolve
				</span>
				<a
					href="/app/housekeeping"
					class="btn btn-primary hk-link"
					data-testid="housekeeping-banner-link"
				>
					Review
				</a>
			</aside>
		{/if}
	</section>
</main>

<style>
	.graph-page {
		padding: var(--space-4);
		min-block-size: 100dvh;
	}
	.graph-section {
		position: relative;
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-lg);
		overflow: hidden;
		block-size: calc(100dvh - 6rem);
		min-block-size: 22rem;
		background: var(--bg-base);
		box-shadow: var(--shadow-2);
	}
	.graph-canvas {
		position: absolute;
		inset: 0;
	}
	.graph-vignette {
		position: absolute;
		inset: 0;
		pointer-events: none;
		background: radial-gradient(
			120% 80% at 50% 50%,
			transparent 60%,
			rgba(0, 0, 0, 0.45) 100%
		);
	}
	.overlay {
		position: absolute;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: 0.45rem 0.75rem;
		font-size: var(--fs-13);
		color: var(--fg-muted);
		pointer-events: none;
	}
	.overlay-status {
		top: var(--space-3);
		left: var(--space-3);
	}
	.overlay-selected {
		bottom: var(--space-3);
		left: var(--space-3);
		max-inline-size: calc(100% - 1.5rem);
		color: var(--fg);
		font-size: var(--fs-14);
	}
	.overlay-status.warn {
		color: var(--warn);
	}
	.overlay-status.danger {
		color: var(--danger);
	}
	.dot {
		inline-size: 7px;
		block-size: 7px;
		border-radius: 50%;
		background: currentColor;
		flex: 0 0 auto;
	}
	.dot.ok {
		background: var(--success);
		box-shadow: 0 0 8px -1px var(--success);
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
	.selected-dot {
		inline-size: 7px;
		block-size: 7px;
		border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 10px -1px var(--accent-glow);
		flex: 0 0 auto;
	}
	.selected-label {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.selected-label.muted {
		color: var(--fg-muted);
	}
	.overlay-housekeeping {
		top: var(--space-3);
		right: var(--space-3);
		pointer-events: auto;
		color: var(--warn);
		gap: var(--space-3);
		max-inline-size: calc(100% - 1.5rem);
	}
	.hk-dot {
		inline-size: 7px;
		block-size: 7px;
		border-radius: 50%;
		background: var(--warn);
		box-shadow: 0 0 10px -1px var(--warn-soft);
		flex: 0 0 auto;
	}
	.hk-count {
		font-size: var(--fs-13);
		font-weight: 500;
		white-space: nowrap;
	}
	.hk-link {
		min-block-size: 32px;
		padding: 0.3rem 0.7rem;
		font-size: var(--fs-13);
	}
</style>
