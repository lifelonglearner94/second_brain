import { describe, it, expect } from 'vitest';
import { SpatialGraphStore } from '../../src/lib/state/spatial-graph.svelte';
import { EDGE_COLOR } from '../../src/lib/graph/build';
import type { ChatInferenceProposal } from '../../src/lib/api/client';

function proposal(id: number, source: number, target: number): ChatInferenceProposal {
	return {
		id,
		mode: 'structural_inference',
		source_concept_id: source,
		target_concept_id: target,
		proposed_type: 'endangers',
		evidence_path: [],
		rationale: null,
		status: 'endorsed',
		created_at: 1,
		resolved_at: 1,
		snapshot: null
	};
}

function graphWithNodes(...ids: number[]) {
	return {
		nodes: ids.map((id) => ({
			id: String(id),
			label: `c-${id}`,
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

describe('SpatialGraphStore — reactive Spatial View-Graph (ADR-0002 optimistic-merge target)', () => {
	it('starts with an empty view-graph', () => {
		const store = new SpatialGraphStore();
		expect(store.data.nodes).toEqual([]);
		expect(store.data.links).toEqual([]);
	});

	it('setData replaces the view-graph with a built snapshot', () => {
		const store = new SpatialGraphStore();
		store.setData(graphWithNodes(1, 2, 3));
		expect(store.data.nodes).toHaveLength(3);
		expect(store.data.links).toHaveLength(0);
	});

	it('mergeEndorsedEdge optimistically merges an endorsed edge into the view-graph', () => {
		const store = new SpatialGraphStore();
		store.setData(graphWithNodes(1, 2, 3));
		store.mergeEndorsedEdge(proposal(101, 1, 3));
		expect(store.data.links).toHaveLength(1);
		expect(store.data.links[0]).toMatchObject({ source: '1', target: '3', label: 'endangers' });
		expect(store.data.links[0]?.asserted_by).toEqual([101]);
	});

	it('mergeEndorsedEdge is a no-op when the endpoints are absent (defensive against a partial view)', () => {
		const store = new SpatialGraphStore();
		store.setData(graphWithNodes(1, 2));
		store.mergeEndorsedEdge(proposal(101, 1, 99));
		expect(store.data.links).toHaveLength(0);
	});

	it('successive approvals accumulate edges in the view-graph', () => {
		const store = new SpatialGraphStore();
		store.setData(graphWithNodes(1, 2, 3, 10, 12));
		store.mergeEndorsedEdge(proposal(101, 1, 3));
		store.mergeEndorsedEdge(proposal(102, 10, 12));
		expect(store.data.links).toHaveLength(2);
	});
});
