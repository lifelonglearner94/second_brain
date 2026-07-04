import { describe, it, expect, vi } from 'vitest';
import { EndorsementStore, type EndorsementApi, type EndorsementGraphMerge } from '../../src/lib/state/endorsement-queue.svelte';
import type { ChatInferenceProposal } from '../../src/lib/api/client';

const STRUCTURAL_PENDING: ChatInferenceProposal = {
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

const THEMATIC_PENDING: ChatInferenceProposal = {
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

const ALREADY_ENDORSED: ChatInferenceProposal = {
	...STRUCTURAL_PENDING,
	id: 103,
	status: 'endorsed',
	resolved_at: 1_700_000_020
};

const STRUCTURAL_ENDORSED: ChatInferenceProposal = {
	...STRUCTURAL_PENDING,
	status: 'endorsed',
	resolved_at: 1_700_000_030
};

function apiStub(
	getInferenceProposals: EndorsementApi['getInferenceProposals'],
	endorseInferenceProposal: EndorsementApi['endorseInferenceProposal']
): EndorsementApi {
	return { getInferenceProposals, endorseInferenceProposal };
}

function graphStub(): { graph: EndorsementGraphMerge; merged: ChatInferenceProposal[] } {
	const merged: ChatInferenceProposal[] = [];
	return {
		graph: { mergeEndorsedEdge: vi.fn((p: ChatInferenceProposal) => merged.push(p)) },
		merged
	};
}

describe('EndorsementStore — the high-epistemic-weight HITL queue (ADR-0004)', () => {
	it('starts idle with an empty queue', () => {
		const store = new EndorsementStore(
			apiStub(vi.fn(), vi.fn()),
			graphStub().graph
		);
		expect(store.status).toBe('idle');
		expect(store.proposals).toEqual([]);
		expect(store.pending).toEqual([]);
	});

	it('refresh() loads proposals from the backend and flips to loaded', async () => {
		const getInferenceProposals = vi
			.fn<EndorsementApi['getInferenceProposals']>()
			.mockResolvedValue([ALREADY_ENDORSED, THEMATIC_PENDING, STRUCTURAL_PENDING]);
		const store = new EndorsementStore(apiStub(getInferenceProposals, vi.fn()), graphStub().graph);
		await store.refresh();
		expect(getInferenceProposals).toHaveBeenCalledOnce();
		expect(store.status).toBe('loaded');
		expect(store.proposals).toHaveLength(3);
	});

	it('pending shows only status=pending proposals — the queue is what still awaits the user', async () => {
		const getInferenceProposals = vi
			.fn<EndorsementApi['getInferenceProposals']>()
			.mockResolvedValue([ALREADY_ENDORSED, THEMATIC_PENDING, STRUCTURAL_PENDING]);
		const store = new EndorsementStore(apiStub(getInferenceProposals, vi.fn()), graphStub().graph);
		await store.refresh();
		expect(store.pending.map((p) => p.id).sort()).toEqual([101, 102]);
	});

	it('refresh() flips to error and surfaces the message when the fetch rejects (e.g. 401)', async () => {
		const getInferenceProposals = vi
			.fn<EndorsementApi['getInferenceProposals']>()
			.mockRejectedValue(new Error('GET /chat/inferences failed: 401'));
		const store = new EndorsementStore(apiStub(getInferenceProposals, vi.fn()), graphStub().graph);
		await store.refresh();
		expect(store.status).toBe('error');
		expect(store.error).toMatch(/401/);
		expect(store.pending).toEqual([]);
	});

	describe('approve(id) — Approve Connection → POST endorsement + optimistic merge (ADR-0002/0004)', () => {
		it('POSTs the endorsement, optimistically merges the edge into the Spatial View-Graph, and drops the proposal from the queue', async () => {
			const getInferenceProposals = vi
				.fn<EndorsementApi['getInferenceProposals']>()
				.mockResolvedValue([STRUCTURAL_PENDING, THEMATIC_PENDING]);
			const endorseInferenceProposal = vi
				.fn<EndorsementApi['endorseInferenceProposal']>()
				.mockResolvedValue(STRUCTURAL_ENDORSED);
			const { graph, merged } = graphStub();
			const store = new EndorsementStore(apiStub(getInferenceProposals, endorseInferenceProposal), graph);
			await store.refresh();

			await store.approve(101);

			expect(endorseInferenceProposal).toHaveBeenCalledWith(101);
			expect(merged).toEqual([STRUCTURAL_ENDORSED]);
			expect(graph.mergeEndorsedEdge).toHaveBeenCalledWith(STRUCTURAL_ENDORSED);
			expect(store.pending.find((p) => p.id === 101)).toBeUndefined();
		});

		it('leaves the other pending proposals untouched when one is approved', async () => {
			const getInferenceProposals = vi
				.fn<EndorsementApi['getInferenceProposals']>()
				.mockResolvedValue([STRUCTURAL_PENDING, THEMATIC_PENDING]);
			const endorseInferenceProposal = vi
				.fn<EndorsementApi['endorseInferenceProposal']>()
				.mockResolvedValue(STRUCTURAL_ENDORSED);
			const store = new EndorsementStore(
				apiStub(getInferenceProposals, endorseInferenceProposal),
				graphStub().graph
			);
			await store.refresh();

			await store.approve(101);

			expect(store.pending.map((p) => p.id)).toEqual([102]);
		});

		it('keeps the proposal in the queue and surfaces an error when the endorsement POST rejects (e.g. 409)', async () => {
			const getInferenceProposals = vi
				.fn<EndorsementApi['getInferenceProposals']>()
				.mockResolvedValue([STRUCTURAL_PENDING]);
			const endorseInferenceProposal = vi
				.fn<EndorsementApi['endorseInferenceProposal']>()
				.mockRejectedValue(new Error('POST /chat/inferences/101/endorse failed: 409'));
			const { graph, merged } = graphStub();
			const store = new EndorsementStore(apiStub(getInferenceProposals, endorseInferenceProposal), graph);
			await store.refresh();

			await expect(store.approve(101)).rejects.toThrow(/409/);
			expect(merged).toHaveLength(0);
			expect(graph.mergeEndorsedEdge).not.toHaveBeenCalled();
			expect(store.pending.find((p) => p.id === 101)).toBeDefined();
		});

		it('does not optimistically merge when the endorsed proposal is not in the queue (defensive)', async () => {
			const getInferenceProposals = vi
				.fn<EndorsementApi['getInferenceProposals']>()
				.mockResolvedValue([STRUCTURAL_PENDING]);
			const endorseInferenceProposal = vi
				.fn<EndorsementApi['endorseInferenceProposal']>()
				.mockResolvedValue(STRUCTURAL_ENDORSED);
			const { graph, merged } = graphStub();
			const store = new EndorsementStore(apiStub(getInferenceProposals, endorseInferenceProposal), graph);
			await store.refresh();

			await store.approve(999);

			expect(endorseInferenceProposal).toHaveBeenCalledWith(999);
			expect(merged).toEqual([STRUCTURAL_ENDORSED]);
		});
	});
});
