import type { GlobalTopologySnapshot, GraphDelta } from '$lib/api/client';

export function applyDelta(
	snapshot: GlobalTopologySnapshot,
	delta: GraphDelta
): GlobalTopologySnapshot {
	const concepts = mergeById(snapshot.concepts, delta.added_concepts);
	const edges = mergeById(snapshot.edges, delta.added_edges);

	const deletedConcepts = new Set(delta.deleted_concept_ids);
	const deletedEdges = new Set(delta.deleted_edge_ids);

	const keptConcepts = concepts.filter((c) => !deletedConcepts.has(c.id));
	const keptEdges = edges.filter(
		(e) => !deletedEdges.has(e.id) && !deletedConcepts.has(e.source_concept_id) && !deletedConcepts.has(e.target_concept_id)
	);
	const keptPartitions = snapshot.partitions.filter((p) => !deletedConcepts.has(p.concept_id));

	const retagById = new Map(delta.retagged_edges.map((r) => [r.id, r]));
	const retaggedEdges = keptEdges.map((e) => {
		const retag = retagById.get(e.id);
		return retag ? { ...e, current_type: retag.current_type } : e;
	});

	return {
		concepts: keptConcepts,
		edges: retaggedEdges,
		partitions: keptPartitions
	};
}

function mergeById<T extends { id: string }>(existing: T[], additions: T[]): T[] {
	const seen = new Set(existing.map((x) => x.id));
	const merged = [...existing];
	for (const add of additions) {
		if (seen.has(add.id)) continue;
		seen.add(add.id);
		merged.push(add);
	}
	return merged;
}
