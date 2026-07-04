import type { GlobalTopologySnapshot, GraphConcept, GraphEdge } from '$lib/api/client';

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
