import { describe, it, expect } from 'vitest';
import { mergeEndorsedEdge } from '../../src/lib/endorsement/merge';
import { EDGE_COLOR, type GraphData } from '../../src/lib/graph/build';
import type { ChatInferenceProposal } from '../../src/lib/api/client';

function graphWith(...conceptIds: number[]): GraphData {
	return {
		nodes: conceptIds.map((id) => ({
			id: String(id),
			label: `concept-${id}`,
			group: 0,
			color: EDGE_COLOR,
			x: 0,
			y: 0,
			z: 0,
			fx: 0,
			fy: 0,
			fz: 0
		})),
		links: []
	};
}

const STRUCTURAL: ChatInferenceProposal = {
	id: 101,
	mode: 'structural_inference',
	source_concept_id: 1,
	target_concept_id: 3,
	proposed_type: 'endangers',
	evidence_path: [
		{ source_concept_id: 1, edge_type: 'endangers', target_concept_id: 2 },
		{ source_concept_id: 2, edge_type: 'depends_on', target_concept_id: 3 }
	],
	rationale: 'Maria endangers the launch the beta depends on',
	status: 'endorsed',
	created_at: 1_700_000_000,
	resolved_at: 1_700_000_030,
	snapshot: null
};

const THEMATIC: ChatInferenceProposal = {
	id: 102,
	mode: 'thematic_inference',
	source_concept_id: 10,
	target_concept_id: 12,
	proposed_type: 'correlates_with',
	evidence_path: [],
	rationale: 'Cluster density suggests a bridge',
	status: 'endorsed',
	created_at: 1_700_000_010,
	resolved_at: 1_700_000_030,
	snapshot: {
		id: 55,
		braindump_ids: [201, 202, 203],
		concept_ids: [10, 11, 12],
		captured_at: 1_700_000_010
	}
};

describe('mergeEndorsedEdge — action-driven optimistic merge into the Spatial View-Graph (ADR-0002/0004)', () => {
	it('adds the endorsed edge as a link spanning source → target with the proposed type', () => {
		const graph = graphWith(1, 2, 3);
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links).toHaveLength(1);
		expect(merged.links[0]).toMatchObject({
			source: '1',
			target: '3',
			label: 'endangers'
		});
	});

	it('origin-tags the merged edge with asserted_by: [Chat_Inference_ID] provenance', () => {
		const graph = graphWith(1, 2, 3);
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links[0]?.asserted_by).toEqual([101]);
	});

	it('colors the optimistically merged edge like every other edge (EDGE_COLOR)', () => {
		const graph = graphWith(1, 2, 3);
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links[0]?.color).toBe(EDGE_COLOR);
	});

	it('merges a thematic proposal the same way — the edge bridges the two cluster-mates', () => {
		const graph = graphWith(10, 11, 12);
		const merged = mergeEndorsedEdge(graph, THEMATIC);
		expect(merged.links).toHaveLength(1);
		expect(merged.links[0]).toMatchObject({
			source: '10',
			target: '12',
			label: 'correlates_with'
		});
		expect(merged.links[0]?.asserted_by).toEqual([102]);
	});

	it('is a no-op when the source concept is absent from the view-graph (defensive against partial views)', () => {
		const graph = graphWith(2, 3);
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links).toHaveLength(0);
	});

	it('is a no-op when the target concept is absent from the view-graph', () => {
		const graph = graphWith(1, 2);
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links).toHaveLength(0);
	});

	it('does not duplicate the edge when the same direct edge already exists in the view-graph', () => {
		const graph = graphWith(1, 2, 3);
		graph.links.push({ source: '1', target: '3', label: 'endangers', color: EDGE_COLOR });
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.links.filter((l) => l.source === '1' && l.target === '3')).toHaveLength(1);
	});

	it('does not mutate the input view-graph (the Spatial View-Graph is replaced, not patched in place)', () => {
		const graph = graphWith(1, 2, 3);
		const before = JSON.stringify(graph);
		mergeEndorsedEdge(graph, STRUCTURAL);
		expect(JSON.stringify(graph)).toBe(before);
	});

	it('preserves every existing node and link when merging a new edge', () => {
		const graph = graphWith(1, 2, 3);
		graph.links.push({ source: '1', target: '2', label: 'endangers', color: EDGE_COLOR });
		const merged = mergeEndorsedEdge(graph, STRUCTURAL);
		expect(merged.nodes).toHaveLength(3);
		expect(merged.links).toHaveLength(2);
		expect(merged.links.some((l) => l.source === '1' && l.target === '2')).toBe(true);
		expect(merged.links.some((l) => l.source === '1' && l.target === '3')).toBe(true);
	});
});
