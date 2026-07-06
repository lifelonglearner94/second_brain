import { describe, it, expect } from 'vitest';
import { MultiDirectedGraph } from 'graphology';
import {
	buildSpatialViewGraph,
	projectToGraphData
} from '../../src/lib/graph/build';
import { NO_PARTITION, partitionColor } from '../../src/lib/graph/colors';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: 'c2', label: 'melatonin', created_at: '2026-07-02T00:00:00Z' },
		{ id: 'c3', label: 'caffeine', created_at: '2026-07-03T00:00:00Z' },
		{ id: 'c4', label: 'orphan', created_at: '2026-07-04T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: 'c1',
			target_concept_id: 'c2',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-02T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: 'c1', partition_id: 0 },
		{ concept_id: 'c2', partition_id: 0 },
		{ concept_id: 'c3', partition_id: 1 }
	]
};

describe('buildSpatialViewGraph — the canonical graphology Spatial View-Graph', () => {
	it('returns a graphology MultiDirectedGraph with one node per concept', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		expect(graph).toBeInstanceOf(MultiDirectedGraph);
		expect(graph.order).toBe(4);
		expect(graph.size).toBe(1);
	});

	it('carries the concept label on each node', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		expect(graph.getNodeAttribute('c1', 'label')).toBe('sleep');
		expect(graph.getNodeAttribute('c4', 'label')).toBe('orphan');
	});

	it('stamps each node with its backend Louvain partition_id (ADR-0008: not computed client-side)', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		expect(graph.getNodeAttribute('c1', 'partition')).toBe(0);
		expect(graph.getNodeAttribute('c2', 'partition')).toBe(0);
		expect(graph.getNodeAttribute('c3', 'partition')).toBe(1);
		expect(graph.getNodeAttribute('c4', 'partition')).toBe(NO_PARTITION);
	});

	it('stores the cluster color on the model so renderers do not recompute it (no duplication)', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		expect(graph.getNodeAttribute('c1', 'color')).toBe(partitionColor(0));
		expect(graph.getNodeAttribute('c3', 'color')).toBe(partitionColor(1));
		expect(graph.getNodeAttribute('c4', 'color')).toBe(
			partitionColor(NO_PARTITION)
		);
	});

	it('runs ForceAtlas2 locally so every node has finite x/y', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		graph.forEachNode((_, attrs) => {
			expect(Number.isFinite(attrs.x)).toBe(true);
			expect(Number.isFinite(attrs.y)).toBe(true);
		});
	});

	it('derives z from the partition so clusters separate (same partition ⇒ same z)', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		expect(graph.getNodeAttribute('c1', 'z')).toBe(
			graph.getNodeAttribute('c2', 'z')
		);
		expect(graph.getNodeAttribute('c1', 'z')).not.toBe(
			graph.getNodeAttribute('c3', 'z')
		);
	});

	it('preserves edge direction and current_type as the edge label', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		const edges = graph.mapEdges((_, attrs) => attrs);
		expect(edges).toHaveLength(1);
		expect(edges[0]?.label).toBe('affects');
	});

	it('handles an empty snapshot as an empty graph', () => {
		const graph = buildSpatialViewGraph({
			concepts: [],
			edges: [],
			partitions: []
		});
		expect(graph.order).toBe(0);
		expect(graph.size).toBe(0);
	});

	it('projectToGraphData reads color/x/y/z from the model (no recompute), so the renderer swap does not duplicate the data model', () => {
		const graph = buildSpatialViewGraph(SNAPSHOT);
		const data = projectToGraphData(graph);
		expect(data.nodes).toHaveLength(graph.order);
		for (const node of data.nodes) {
			expect(node.color).toBe(graph.getNodeAttribute(node.id, 'color'));
			expect(node.x).toBe(graph.getNodeAttribute(node.id, 'x'));
			expect(node.y).toBe(graph.getNodeAttribute(node.id, 'y'));
			expect(node.z).toBe(graph.getNodeAttribute(node.id, 'z'));
		}
	});
});
