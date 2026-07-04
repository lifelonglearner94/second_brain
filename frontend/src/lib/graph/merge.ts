import type {
	GlobalTopologySnapshot,
	GraphConcept,
	GraphEdge,
	ConceptMergeSuggestion,
	OntologyTypeProposal
} from '$lib/api/client';

export type IngestedGraph = {
	concepts: GraphConcept[];
	edges: GraphEdge[];
};

export function mergeIntoGraph(
	snapshot: GlobalTopologySnapshot,
	ingested: IngestedGraph
): GlobalTopologySnapshot {
	const existingConceptIds = new Set(snapshot.concepts.map((c) => c.id));
	const existingEdgeIds = new Set(snapshot.edges.map((e) => e.id));

	const newConcepts = ingested.concepts.filter((c) => !existingConceptIds.has(c.id));
	const newEdges = ingested.edges.filter((e) => !existingEdgeIds.has(e.id));

	if (newConcepts.length === 0 && newEdges.length === 0) {
		return snapshot;
	}

	return {
		concepts: [...snapshot.concepts, ...newConcepts],
		edges: [...snapshot.edges, ...newEdges],
		partitions: snapshot.partitions
	};
}

export function applyConceptMerge(
	snapshot: GlobalTopologySnapshot,
	suggestion: ConceptMergeSuggestion
): GlobalTopologySnapshot {
	const foldId = String(suggestion.new_concept_id);
	const keepId = String(suggestion.existing_concept_id);
	if (foldId === keepId) return snapshot;
	if (!snapshot.concepts.some((c) => c.id === foldId)) return snapshot;

	const concepts = snapshot.concepts.filter((c) => c.id !== foldId);
	const partitions = snapshot.partitions.filter((p) => p.concept_id !== foldId);

	const edges = [];
	for (const edge of snapshot.edges) {
		const source = edge.source_concept_id === foldId ? keepId : edge.source_concept_id;
		const target = edge.target_concept_id === foldId ? keepId : edge.target_concept_id;
		if (source === target) continue;
		edges.push({ ...edge, source_concept_id: source, target_concept_id: target });
	}

	return { concepts, edges, partitions };
}

export function applyTypeMerge(
	snapshot: GlobalTopologySnapshot,
	proposal: OntologyTypeProposal
): GlobalTopologySnapshot {
	if (!proposal.merge_of) return snapshot;
	const from = proposal.merge_of;
	const to = proposal.slug;
	if (from === to) return snapshot;
	if (!snapshot.edges.some((e) => e.current_type === from)) return snapshot;

	const edges = snapshot.edges.map((edge) =>
		edge.current_type === from ? { ...edge, current_type: to } : edge
	);
	return { ...snapshot, edges };
}
