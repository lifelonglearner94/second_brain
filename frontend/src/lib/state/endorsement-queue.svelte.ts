import { apiClient } from '$lib/api';
import type { ChatInferenceProposal } from '$lib/api/client';
import { graphStore } from '$lib/state/graph.svelte';

export type EndorsementStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type EndorsementApi = {
	getInferenceProposals(): Promise<ChatInferenceProposal[]>;
	endorseInferenceProposal(id: number): Promise<ChatInferenceProposal>;
};

export type EndorsementGraphMerge = {
	mergeEndorsedEdge(proposal: ChatInferenceProposal): void;
};

export class EndorsementStore {
	status = $state<EndorsementStatus>('idle');
	proposals = $state<ChatInferenceProposal[]>([]);
	error = $state<string | null>(null);

	constructor(
		private api: EndorsementApi,
		private graph: EndorsementGraphMerge
	) {}

	pending = $derived.by<ChatInferenceProposal[]>(() =>
		this.proposals.filter((p) => p.status === 'pending')
	);

	async refresh(): Promise<void> {
		this.status = 'loading';
		this.error = null;
		try {
			this.proposals = await this.api.getInferenceProposals();
			this.status = 'loaded';
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
		}
	}

	async approve(id: number): Promise<ChatInferenceProposal> {
		const endorsed = await this.api.endorseInferenceProposal(id);
		this.graph.mergeEndorsedEdge(endorsed);
		this.proposals = this.proposals.map((p) =>
			p.id === endorsed.id ? endorsed : p
		);
		return endorsed;
	}
}

export const endorsementQueue = new EndorsementStore(apiClient, graphStore);
