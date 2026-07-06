// @vitest-environment jsdom
// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import {
	render,
	screen,
	fireEvent,
	cleanup,
	within
} from '@testing-library/svelte';
import EndorsementQueue from '../../src/lib/endorsement/EndorsementQueue.svelte';
import { mergeEndorsedEdge } from '../../src/lib/endorsement/merge';
import { EDGE_COLOR, type GraphData } from '../../src/lib/graph/build';
import type { ChatInferenceProposal } from '../../src/lib/api/client';

const LABELS = new Map<number, string>([
	[1, 'Maria'],
	[2, 'Q3 launch'],
	[3, 'Beta release'],
	[10, 'Burnout'],
	[11, 'Sleep'],
	[12, 'Caffeine']
]);

function labelFor(id: number): string | null {
	return LABELS.get(id) ?? null;
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
	status: 'pending',
	created_at: 1_700_000_000,
	resolved_at: null,
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
	status: 'pending',
	created_at: 1_700_000_010,
	resolved_at: null,
	snapshot: {
		id: 55,
		braindump_ids: [201, 202, 203],
		concept_ids: [10, 11, 12],
		captured_at: 1_700_000_010
	}
};

function emptyGraph(): GraphData {
	return {
		nodes: [1, 2, 3, 10, 11, 12].map((id) => ({
			id: String(id),
			label: labelFor(id) ?? String(id),
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

afterEach(() => {
	cleanup();
});

describe('EndorsementQueue — Evidence Disclosure surface (ADR-0004/0009)', () => {
	it('lists every pending chat-inferred edge proposal from the backend', () => {
		render(EndorsementQueue, {
			props: {
				proposals: [STRUCTURAL, THEMATIC],
				labelFor,
				onApproveConnection: vi.fn()
			}
		});
		const items = screen.getAllByTestId(/^endorsement-item-/);
		expect(items).toHaveLength(2);
		expect(screen.getByTestId('endorsement-item-101')).toBeTruthy();
		expect(screen.getByTestId('endorsement-item-102')).toBeTruthy();
	});

	it('uses the action verb "Approve Connection" — never "Merge"', () => {
		render(EndorsementQueue, {
			props: {
				proposals: [STRUCTURAL, THEMATIC],
				labelFor,
				onApproveConnection: vi.fn()
			}
		});
		const verbs = screen.getAllByTestId('approve-connection');
		expect(verbs).toHaveLength(2);
		for (const btn of verbs) {
			expect(btn.textContent).toBe('Approve Connection');
		}
		expect(screen.queryByTestId('merge-button')).toBeNull();
		expect(screen.queryByText('Merge')).toBeNull();
	});

	it('shows NO academic "Structural"/"Thematic" type labels — the distinction is the evidence payload, not a name', () => {
		render(EndorsementQueue, {
			props: {
				proposals: [STRUCTURAL, THEMATIC],
				labelFor,
				onApproveConnection: vi.fn()
			}
		});
		expect(screen.queryByText('Structural')).toBeNull();
		expect(screen.queryByText('Thematic')).toBeNull();
		expect(screen.queryByText('Structural Inference')).toBeNull();
		expect(screen.queryByText('Thematic Inference')).toBeNull();
		expect(screen.queryByText('structural_inference')).toBeNull();
		expect(screen.queryByText('thematic_inference')).toBeNull();
	});

	it('renders the proposed connection readably using concept labels (Maria —[endangers]→ Beta release)', () => {
		render(EndorsementQueue, {
			props: { proposals: [STRUCTURAL], labelFor, onApproveConnection: vi.fn() }
		});
		const head = screen.getByTestId('proposed-connection-101');
		expect(head.textContent).toContain('Maria');
		expect(head.textContent).toContain('Beta release');
		expect(head.textContent).toContain('endangers');
	});

	describe('expand-evidence — structural proposal', () => {
		it('discloses the heading "Based on existing path" and, on expand, the traversable node-edge-node chain', async () => {
			render(EndorsementQueue, {
				props: {
					proposals: [STRUCTURAL],
					labelFor,
					onApproveConnection: vi.fn()
				}
			});
			const toggle = screen.getByTestId('evidence-toggle-101');
			expect(toggle.textContent).toContain('Based on existing path');

			expect(screen.queryByTestId('evidence-path-101')).toBeNull();
			await fireEvent.click(toggle);

			const path = screen.getByTestId('evidence-path-101');
			expect(path.textContent).toContain('Maria');
			expect(path.textContent).toContain('endangers');
			expect(path.textContent).toContain('Q3 launch');
			expect(path.textContent).toContain('depends_on');
			expect(path.textContent).toContain('Beta release');
		});
	});

	describe('expand-evidence — thematic proposal', () => {
		it('discloses the heading "Based on thematic density" and, on expand, the frozen Thematic Snapshot', async () => {
			render(EndorsementQueue, {
				props: { proposals: [THEMATIC], labelFor, onApproveConnection: vi.fn() }
			});
			const toggle = screen.getByTestId('evidence-toggle-102');
			expect(toggle.textContent).toContain('Based on thematic density');

			expect(screen.queryByTestId('evidence-snapshot-102')).toBeNull();
			await fireEvent.click(toggle);

			const snapshot = screen.getByTestId('evidence-snapshot-102');
			expect(snapshot.textContent).toContain('201');
			expect(snapshot.textContent).toContain('202');
			expect(snapshot.textContent).toContain('203');
		});
	});

	describe('approve → optimistic-merge into the Spatial View-Graph', () => {
		it('fires onApproveConnection with the proposal, whose wiring optimistically merges the edge into the view-graph', async () => {
			let graph = emptyGraph();
			const onApproveConnection = vi.fn(
				async (proposal: ChatInferenceProposal) => {
					const endorsed: ChatInferenceProposal = {
						...proposal,
						status: 'endorsed',
						resolved_at: 1
					};
					graph = mergeEndorsedEdge(graph, endorsed);
				}
			);
			render(EndorsementQueue, {
				props: { proposals: [STRUCTURAL], labelFor, onApproveConnection }
			});

			const item = within(screen.getByTestId('endorsement-item-101'));
			const button = item.getByTestId('approve-connection');
			await fireEvent.click(button);

			expect(onApproveConnection).toHaveBeenCalledWith(STRUCTURAL);
			const merged = graph.links.find(
				(l) => l.source === '1' && l.target === '3' && l.label === 'endangers'
			);
			expect(merged).toBeDefined();
			expect(merged?.asserted_by).toEqual([101]);
		});
	});
});
