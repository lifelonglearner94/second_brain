import { describe, it, expect } from 'vitest';
import { mergeIntoGraph } from '../../src/lib/graph/merge';
import { buildGraphData } from '../../src/lib/graph/build';
import { NO_PARTITION } from '../../src/lib/graph/colors';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [
		{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: 'c2', label: 'melatonin', created_at: '2026-07-02T00:00:00Z' }
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
		{ concept_id: 'c2', partition_id: 0 }
	]
};

describe('mergeIntoGraph — optimistic merge of ingested concepts/edges into the Spatial View-Graph (ADR-0002)', () => {
	it('appends newly-extracted concepts to the snapshot without touching existing ones', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }],
			edges: []
		});
		expect(merged.concepts.map((c) => c.id).sort()).toEqual(['c1', 'c2', 'c3']);
		expect(merged.concepts.find((c) => c.id === 'c1')?.label).toBe('sleep');
	});

	it('appends newly-extracted edges to the snapshot preserving direction and current_type', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }],
			edges: [
				{
					id: 'e2',
					source_concept_id: 'c3',
					target_concept_id: 'c1',
					original_type: 'disrupts',
					current_type: 'disrupts',
					created_at: '2026-07-04T00:00:00Z'
				}
			]
		});
		expect(merged.edges.map((e) => e.id).sort()).toEqual(['e1', 'e2']);
		expect(merged.edges.find((e) => e.id === 'e2')?.current_type).toBe('disrupts');
	});

	it('does not duplicate a concept the Spatial View-Graph already holds (idempotent under re-merge)', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' }],
			edges: []
		});
		expect(merged.concepts.filter((c) => c.id === 'c1')).toHaveLength(1);
	});

	it('does not duplicate an edge the Spatial View-Graph already holds', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [],
			edges: [
				{
					id: 'e1',
					source_concept_id: 'c1',
					target_concept_id: 'c2',
					original_type: 'affects',
					current_type: 'affects',
					created_at: '2026-07-02T00:00:00Z'
				}
			]
		});
		expect(merged.edges.filter((e) => e.id === 'e1')).toHaveLength(1);
	});

	it('leaves the Louvain partitions untouched — new concepts get NO_PARTITION until the next sync (ADR-0008)', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }],
			edges: []
		});
		expect(merged.partitions).toEqual(SNAPSHOT.partitions);
		expect(merged.partitions.find((p) => p.concept_id === 'c3')).toBeUndefined();
	});

	it('does not mutate the input snapshot (pure merge — the next Delta Sync overwrites the view)', () => {
		const before = JSON.parse(JSON.stringify(SNAPSHOT)) as GlobalTopologySnapshot;
		mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: 't' }],
			edges: []
		});
		expect(SNAPSHOT).toEqual(before);
	});

	it('feed through buildGraphData: the new concept renders as a node and the new edge as a link', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: '2026-07-04T00:00:00Z' }],
			edges: [
				{
					id: 'e2',
					source_concept_id: 'c3',
					target_concept_id: 'c1',
					original_type: 'disrupts',
					current_type: 'disrupts',
					created_at: '2026-07-04T00:00:00Z'
				}
			]
		});
		const data = buildGraphData(merged);
		expect(data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2', 'c3']);
		expect(data.links.map((l) => `${l.source}-${l.target}`).sort()).toEqual(['c1-c2', 'c3-c1']);
	});

	it('a freshly-merged concept falls back to NO_PARTITION in the rendered graph (no client-side Louvain)', () => {
		const merged = mergeIntoGraph(SNAPSHOT, {
			concepts: [{ id: 'c3', label: 'caffeine', created_at: 't' }],
			edges: []
		});
		const data = buildGraphData(merged);
		expect(data.nodes.find((n) => n.id === 'c3')?.group).toBe(NO_PARTITION);
	});

	it('an empty ingest is a no-op (the view is unchanged)', () => {
		const merged = mergeIntoGraph(SNAPSHOT, { concepts: [], edges: [] });
		expect(merged).toEqual(SNAPSHOT);
	});
});
