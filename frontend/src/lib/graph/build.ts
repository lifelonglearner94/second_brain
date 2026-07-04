import { MultiDirectedGraph } from 'graphology';
import { random } from 'graphology-layout';
import forceAtlas2 from 'graphology-layout-forceatlas2';
import type { GlobalTopologySnapshot } from '$lib/api/client';
import { NO_PARTITION, partitionColor } from './colors';

export type GraphNode = {
	id: string;
	label: string;
	group: number;
	color: string;
	x: number;
	y: number;
	z: number;
	fx: number;
	fy: number;
	fz: number;
};

export type GraphLink = {
	source: string;
	target: string;
	label: string;
	color: string;
	asserted_by?: number[];
};

export type GraphData = {
	nodes: GraphNode[];
	links: GraphLink[];
};

export const EDGE_COLOR = '#46506a';

const DEFAULT_ITERATIONS = 50;
const DEFAULT_Z_STEP = 40;

export type BuildGraphDataOptions = {
	iterations?: number;
	zStep?: number;
};

export function buildSpatialViewGraph(
	snapshot: GlobalTopologySnapshot,
	options: BuildGraphDataOptions = {}
): MultiDirectedGraph {
	const iterations = options.iterations ?? DEFAULT_ITERATIONS;
	const zStep = options.zStep ?? DEFAULT_Z_STEP;

	const partitionByConcept = new Map<string, number>();
	for (const p of snapshot.partitions) {
		partitionByConcept.set(p.concept_id, p.partition_id);
	}

	const knownConcepts = new Set(snapshot.concepts.map((c) => c.id));

	const graph = new MultiDirectedGraph();
	for (const concept of snapshot.concepts) {
		const group = partitionByConcept.get(concept.id) ?? NO_PARTITION;
		graph.addNode(concept.id, {
			label: concept.label,
			group,
			partition: group,
			color: partitionColor(group)
		});
	}

	for (const edge of snapshot.edges) {
		if (!knownConcepts.has(edge.source_concept_id) || !knownConcepts.has(edge.target_concept_id)) {
			continue;
		}
		if (edge.source_concept_id === edge.target_concept_id) {
			continue;
		}
		graph.addEdge(edge.source_concept_id, edge.target_concept_id, {
			label: edge.current_type
		});
	}

	if (graph.order > 0) {
		random.assign(graph, { scale: 100, center: 0 });
		forceAtlas2.assign(graph, {
			iterations,
			settings: forceAtlas2.inferSettings(graph)
		});
		graph.forEachNode((key, attrs) => {
			const group = (attrs.group as number) ?? NO_PARTITION;
			graph.setNodeAttribute(key, 'z', group * zStep);
		});
	}

	return graph;
}

export function projectToGraphData(
	graph: MultiDirectedGraph
): GraphData {
	const nodes: GraphNode[] = [];
	graph.forEachNode((key, attrs) => {
		const x = attrs.x as number;
		const y = attrs.y as number;
		const z = attrs.z as number;
		nodes.push({
			id: key,
			label: (attrs.label as string) ?? key,
			group: (attrs.group as number) ?? NO_PARTITION,
			color: (attrs.color as string) ?? partitionColor((attrs.group as number) ?? NO_PARTITION),
			x,
			y,
			z,
			fx: x,
			fy: y,
			fz: z
		});
	});

	const links: GraphLink[] = [];
	graph.forEachEdge((key, attrs, source, target) => {
		links.push({
			source,
			target,
			label: (attrs.label as string) ?? '',
			color: EDGE_COLOR
		});
	});

	return { nodes, links };
}

export function buildGraphData(
	snapshot: GlobalTopologySnapshot,
	options: BuildGraphDataOptions = {}
): GraphData {
	if (snapshot.concepts.length === 0) {
		return { nodes: [], links: [] };
	}
	return projectToGraphData(buildSpatialViewGraph(snapshot, options));
}
