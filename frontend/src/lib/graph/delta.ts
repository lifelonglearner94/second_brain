import type { GlobalTopologySnapshot, GraphDelta } from '$lib/api/client';

export function applyDelta(
	snapshot: GlobalTopologySnapshot,
	delta: GraphDelta
): GlobalTopologySnapshot {
	const concepts = mergeById(snapshot.concepts, delta.added_concepts);
	const edges = mergeById(snapshot.edges, delta.added_edges);
	return {
		concepts,
		edges,
		partitions: snapshot.partitions
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
