import { describe, it, expect } from 'vitest';
import { buildGraphData, type GraphData } from '../../src/lib/graph/build';
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
		},
		{
			id: 'e2',
			source_concept_id: 'c3',
			target_concept_id: 'c1',
			original_type: 'disrupts',
			current_type: 'disrupts',
			created_at: '2026-07-03T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: 'c1', partition_id: 0 },
		{ concept_id: 'c2', partition_id: 0 },
		{ concept_id: 'c3', partition_id: 1 }
	]
};

describe('buildGraphData - snapshot → graphology Spatial View-Graph → 3d-force-graph data', () => {
	it('turns every concept into a node carrying its id and label', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		expect(nodes).toHaveLength(4);
		expect(nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2', 'c3', 'c4']);
		const sleep = nodes.find((n) => n.id === 'c1');
		expect(sleep?.label).toBe('sleep');
	});

	it('turns every backend edge into a typed link preserving direction and current_type', () => {
		const { links } = buildGraphData(SNAPSHOT);
		expect(links).toHaveLength(2);
		const e1 = links.find((l) => l.source === 'c1' && l.target === 'c2');
		expect(e1?.label).toBe('affects');
		const e2 = links.find((l) => l.source === 'c3' && l.target === 'c1');
		expect(e2?.label).toBe('disrupts');
	});

	it('assigns each node the Louvain partition_id from the snapshot (ADR-0008: not computed client-side)', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		expect(nodes.find((n) => n.id === 'c1')?.group).toBe(0);
		expect(nodes.find((n) => n.id === 'c2')?.group).toBe(0);
		expect(nodes.find((n) => n.id === 'c3')?.group).toBe(1);
	});

	it('gives concepts with no partition entry the NO_PARTITION fallback', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		expect(nodes.find((n) => n.id === 'c4')?.group).toBe(NO_PARTITION);
	});

	it('colors each node by its partition via partitionColor', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		for (const n of nodes) {
			expect(n.color).toBe(partitionColor(n.group));
		}
	});

	it('runs ForceAtlas2 locally so every node has finite, non-NaN x/y (coordinates not fetched)', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		for (const n of nodes) {
			expect(Number.isFinite(n.x)).toBe(true);
			expect(Number.isFinite(n.y)).toBe(true);
			expect(Number.isFinite(n.z)).toBe(true);
		}
	});

	it('separates connected nodes (ForceAtlas2 repulsion) so no two share the exact x/y', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		const c1 = nodes.find((n) => n.id === 'c1')!;
		const c2 = nodes.find((n) => n.id === 'c2')!;
		expect(c1.x === c2.x && c1.y === c2.y).toBe(false);
	});

	it('fixes positions (fx/fy/fz) so the renderer honors the locally-computed layout, not its own physics', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		for (const n of nodes) {
			expect(n.fx).toBe(n.x);
			expect(n.fy).toBe(n.y);
			expect(n.fz).toBe(n.z);
		}
	});

	it('derives z from the backend partition_id so clusters separate in 3D (same partition ⇒ same z)', () => {
		const { nodes } = buildGraphData(SNAPSHOT);
		const c1 = nodes.find((n) => n.id === 'c1')!;
		const c2 = nodes.find((n) => n.id === 'c2')!;
		const c3 = nodes.find((n) => n.id === 'c3')!;
		expect(c1.z).toBe(c2.z);
		expect(c1.z).not.toBe(c3.z);
	});

	it('drops edges whose endpoint is missing from concepts (defensive against partial snapshots)', () => {
		const partial: GlobalTopologySnapshot = {
			concepts: [{ id: 'c1', label: 'solo', created_at: 't' }],
			edges: [
				{
					id: 'e1',
					source_concept_id: 'c1',
					target_concept_id: 'ghost',
					original_type: 'affects',
					current_type: 'affects',
					created_at: 't'
				}
			],
			partitions: []
		};
		expect(buildGraphData(partial).links).toHaveLength(0);
	});

	it('handles an empty snapshot as empty data', () => {
		const empty: GraphData = buildGraphData({
			concepts: [],
			edges: [],
			partitions: []
		});
		expect(empty.nodes).toEqual([]);
		expect(empty.links).toEqual([]);
	});
});
