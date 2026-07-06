// @vitest-environment jsdom
import { describe, it, expect, vi } from 'vitest';
import { MultiDirectedGraph } from 'graphology';
import {
	renderSpatialViewGraph2D,
	HIGHLIGHT_2D,
	type SigmaLike
} from '../../src/lib/graph/render2d';
import { buildSpatialViewGraph } from '../../src/lib/graph/build';
import { partitionColor, NO_PARTITION } from '../../src/lib/graph/colors';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

type Handler = (payload: unknown) => void;

class FakeSigma implements SigmaLike {
	graph: unknown;
	container: HTMLElement;
	settings: Record<string, unknown>;
	nodeReducer:
		| ((node: string, data: Record<string, unknown>) => Record<string, unknown>)
		| null = null;
	private handlers = new Map<string, Handler[]>();
	refreshed = 0;
	killed = false;

	constructor(
		graph: unknown,
		container: HTMLElement,
		settings: Record<string, unknown>
	) {
		this.graph = graph;
		this.container = container;
		this.settings = settings;
	}
	on(event: string, handler: Handler): this {
		const list = this.handlers.get(event) ?? [];
		list.push(handler);
		this.handlers.set(event, list);
		return this;
	}
	setSetting(key: string, value: unknown): this {
		if (key === 'nodeReducer') {
			this.nodeReducer = value as this['nodeReducer'];
		}
		return this;
	}
	refresh(): this {
		this.refreshed += 1;
		return this;
	}
	kill(): void {
		this.killed = true;
	}
	emit(event: string, payload: unknown): void {
		for (const h of this.handlers.get(event) ?? []) h(payload);
	}
}

function makeGraph(): MultiDirectedGraph {
	const g = new MultiDirectedGraph();
	g.addNode('c1', {
		label: 'sleep',
		group: 0,
		partition: 0,
		color: partitionColor(0),
		x: 1,
		y: 2,
		z: 0
	});
	g.addNode('c2', {
		label: 'caffeine',
		group: 1,
		partition: 1,
		color: partitionColor(1),
		x: 3,
		y: 4,
		z: 40
	});
	g.addEdge('c1', 'c2', { label: 'disrupts' });
	return g;
}

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: 'c2', label: 'caffeine', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: 'c1',
			target_concept_id: 'c2',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '2026-07-02T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: 'c1', partition_id: 0 },
		{ concept_id: 'c2', partition_id: 1 }
	]
};

describe('renderSpatialViewGraph2D — sigma.js v3 2D WebGL fallback over the same graphology model', () => {
	it('constructs sigma with the SAME graphology instance (renderer swap does not duplicate the data model)', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		let captured: unknown = null;
		const factory = vi.fn((g: unknown) => {
			captured = g;
			return new FakeSigma(g, container, {});
		});
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode: vi.fn(),
			sigmaFactory: factory
		});
		expect(captured).toBe(graph);
	});

	it('colors the selected node with the highlight color and leaves other nodes on their cluster color', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		const fake = new FakeSigma(graph, container, {});
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: 'c1',
			onSelectNode: vi.fn(),
			sigmaFactory: () => fake
		});
		expect(fake.nodeReducer).not.toBeNull();
		const sleepData = {
			color: graph.getNodeAttribute('c1', 'color'),
			label: 'sleep'
		};
		const cafData = {
			color: graph.getNodeAttribute('c2', 'color'),
			label: 'caffeine'
		};
		expect(fake.nodeReducer!('c1', sleepData).color).toBe(HIGHLIGHT_2D);
		expect(fake.nodeReducer!('c2', cafData).color).toBe(
			graph.getNodeAttribute('c2', 'color')
		);
	});

	it('reports clickNode as a selection (id + label) from the graphology model', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		const fake = new FakeSigma(graph, container, {});
		const onSelectNode = vi.fn();
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode,
			sigmaFactory: () => fake
		});
		fake.emit('clickNode', { node: 'c2' });
		expect(onSelectNode).toHaveBeenCalledWith('c2', 'caffeine');
	});

	it('reports clickStage as a de-selection (null, null)', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		const fake = new FakeSigma(graph, container, {});
		const onSelectNode = vi.fn();
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode,
			sigmaFactory: () => fake
		});
		fake.emit('clickStage', { event: {} });
		expect(onSelectNode).toHaveBeenCalledWith(null, null);
	});

	it('setSelected re-targets the highlight and refreshes sigma so the new selection paints', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		const fake = new FakeSigma(graph, container, {});
		const handle = await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode: vi.fn(),
			sigmaFactory: () => fake
		});
		handle.setSelected('c2');
		expect(fake.refreshed).toBeGreaterThan(0);
		const cafData = {
			color: graph.getNodeAttribute('c2', 'color'),
			label: 'caffeine'
		};
		const sleepData = {
			color: graph.getNodeAttribute('c1', 'color'),
			label: 'sleep'
		};
		expect(fake.nodeReducer!('c2', cafData).color).toBe(HIGHLIGHT_2D);
		expect(fake.nodeReducer!('c1', sleepData).color).toBe(
			graph.getNodeAttribute('c1', 'color')
		);
	});

	it('destroy() kills sigma so the canvas/WebGL resources are released', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		const fake = new FakeSigma(graph, container, {});
		const handle = await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode: vi.fn(),
			sigmaFactory: () => fake
		});
		handle.destroy();
		expect(fake.killed).toBe(true);
	});

	it('a node with NO_PARTITION keeps its fallback cluster color through the reducer (cluster feeling preserved)', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		graph.addNode('c9', {
			label: 'orphan',
			group: NO_PARTITION,
			partition: NO_PARTITION,
			color: partitionColor(NO_PARTITION),
			x: 5,
			y: 6,
			z: -40
		});
		const fake = new FakeSigma(graph, container, {});
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: 'c1',
			onSelectNode: vi.fn(),
			sigmaFactory: () => fake
		});
		const orphanData = {
			color: graph.getNodeAttribute('c9', 'color'),
			label: 'orphan'
		};
		expect(fake.nodeReducer!('c9', orphanData).color).toBe(
			partitionColor(NO_PARTITION)
		);
	});

	it('builds the Spatial View-Graph with a tappable node size on every concept (issue #57: nodes too small to tap on mobile)', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		const sizes: number[] = [];
		graph.forEachNode((_id, attrs) => {
			sizes.push(attrs.size as number);
		});
		expect(sizes).toHaveLength(2);
		for (const size of sizes) {
			expect(typeof size).toBe('number');
			expect(size).toBeGreaterThanOrEqual(6);
		}
	});

	it('configures sigma with a lowered labelRenderedSizeThreshold so node labels render on mobile (issue #57: labels suppressed at threshold 6)', async () => {
		const container = document.createElement('div');
		const graph = makeGraph();
		let capturedSettings: Record<string, unknown> | undefined;
		const factory = vi.fn(
			(g: unknown, _c: HTMLElement, settings?: Record<string, unknown>) => {
				capturedSettings = settings;
				return new FakeSigma(g, container, {});
			}
		);
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: null,
			onSelectNode: vi.fn(),
			sigmaFactory: factory
		});
		expect(capturedSettings).toBeDefined();
		const threshold = capturedSettings!.labelRenderedSizeThreshold;
		expect(typeof threshold).toBe('number');
		expect(threshold as number).toBeLessThan(6);
	});

	it('preserves the larger node size and white selected-node highlight through the reducer (tappable + cluster coloring intact)', async () => {
		const container = document.createElement('div');
		const graph = buildSpatialViewGraph(SNAPSHOT);
		const fake = new FakeSigma(graph, container, {});
		await renderSpatialViewGraph2D(container, graph, {
			selectedNodeId: 'c1',
			onSelectNode: vi.fn(),
			sigmaFactory: () => fake
		});
		expect(fake.nodeReducer).not.toBeNull();
		const sleepData = {
			color: graph.getNodeAttribute('c1', 'color') as string,
			label: graph.getNodeAttribute('c1', 'label') as string,
			size: graph.getNodeAttribute('c1', 'size') as number
		};
		const cafData = {
			color: graph.getNodeAttribute('c2', 'color') as string,
			label: graph.getNodeAttribute('c2', 'label') as string,
			size: graph.getNodeAttribute('c2', 'size') as number
		};
		const sleepOut = fake.nodeReducer!('c1', sleepData);
		const cafOut = fake.nodeReducer!('c2', cafData);
		expect(sleepOut.color).toBe(HIGHLIGHT_2D);
		expect(cafOut.color).toBe(graph.getNodeAttribute('c2', 'color'));
		expect(sleepOut.size).toBe(sleepData.size);
		expect(cafOut.size).toBe(cafData.size);
	});
});
