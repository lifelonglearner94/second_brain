import { apiClient } from '$lib/api';
import type {
	GlobalTopologySnapshot,
	ConceptMergeSuggestion,
	Ontology,
	OntologyTypeProposal,
	OntologyProposalsResponse,
	OntologyEdgeType
} from '$lib/api/client';
import { graphStore, type GraphStore } from '$lib/state/graph.svelte';

export type HousekeepingStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type HousekeepingApi = {
	getMergeSuggestions(): Promise<ConceptMergeSuggestion[]>;
	approveMergeSuggestion(id: number): Promise<void>;
	getOntology(): Promise<Ontology>;
	getOntologyProposals(): Promise<OntologyProposalsResponse>;
	approveOntologyProposal(id: number): Promise<OntologyTypeProposal>;
};

export type HousekeepingItemKind = 'concept' | 'type';

export type HousekeepingItem = {
	id: number;
	kind: HousekeepingItemKind;
	leftLabel: string;
	rightLabel: string;
	similarity: number;
};

export class HousekeepingStore {
	status = $state<HousekeepingStatus>('idle');
	error = $state<string | null>(null);
	ontology = $state<Ontology | null>(null);
	conceptSuggestions = $state<ConceptMergeSuggestion[]>([]);
	typeProposals = $state<OntologyTypeProposal[]>([]);

	constructor(
		private api: HousekeepingApi,
		private graph: GraphStore
	) {}

	get snapshot(): GlobalTopologySnapshot | null {
		return this.graph.snapshot;
	}

	async load(): Promise<void> {
		this.status = 'loading';
		this.error = null;
		try {
			const [suggestions, proposalsRes, ontology] = await Promise.all([
				this.api.getMergeSuggestions(),
				this.api.getOntologyProposals(),
				this.api.getOntology()
			]);
			this.conceptSuggestions = suggestions;
			this.typeProposals = proposalsRes.proposals;
			this.ontology = ontology;
			this.status = 'loaded';
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
			this.conceptSuggestions = [];
			this.typeProposals = [];
		}
	}

	items = $derived.by<HousekeepingItem[]>(() => {
		const labelById = new Map<string, string>();
		for (const c of this.graph.snapshot?.concepts ?? [])
			labelById.set(c.id, c.label);
		const labelBySlug = new Map<string, string>();
		for (const t of this.ontology?.edge_types ?? [])
			labelBySlug.set(t.slug, t.label);

		const concepts: HousekeepingItem[] = this.conceptSuggestions.map((s) => ({
			id: s.id,
			kind: 'concept',
			leftLabel: s.new_concept_label,
			rightLabel:
				labelById.get(String(s.existing_concept_id)) ??
				String(s.existing_concept_id),
			similarity: s.similarity
		}));
		const types: HousekeepingItem[] = this.typeProposals
			.filter((p) => p.near_match_slug !== null)
			.map((p) => ({
				id: p.id,
				kind: 'type',
				leftLabel: p.label,
				rightLabel: labelBySlug.get(p.near_match_slug!) ?? p.near_match_slug!,
				similarity: p.near_match_similarity ?? 0
			}));
		return [...concepts, ...types];
	});

	async confirmMerge(id: number, kind: HousekeepingItemKind): Promise<void> {
		if (kind === 'concept') {
			const suggestion = this.conceptSuggestions.find((s) => s.id === id);
			if (!suggestion || !this.graph.snapshot) return;
			await this.api.approveMergeSuggestion(id);
			this.graph.applyConceptMerge(suggestion);
			this.conceptSuggestions = this.conceptSuggestions.filter(
				(s) => s.id !== id
			);
		} else {
			const proposal = this.typeProposals.find((p) => p.id === id);
			if (!proposal || !this.graph.snapshot) return;
			const approved = await this.api.approveOntologyProposal(id);
			this.graph.applyTypeMerge(approved);
			this.ontology = this.addTypeToOntology(this.ontology, approved);
			this.typeProposals = this.typeProposals.filter((p) => p.id !== id);
		}
	}

	private addTypeToOntology(
		ontology: Ontology | null,
		proposal: OntologyTypeProposal
	): Ontology | null {
		if (!ontology) return ontology;
		if (ontology.edge_types.some((t) => t.slug === proposal.slug))
			return ontology;
		const edgeType: OntologyEdgeType = {
			slug: proposal.slug,
			label: proposal.label,
			description: proposal.description
		};
		return { edge_types: [...ontology.edge_types, edgeType] };
	}
}

export const housekeeping = new HousekeepingStore(apiClient, graphStore);
