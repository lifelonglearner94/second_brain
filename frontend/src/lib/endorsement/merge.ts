import { EDGE_COLOR, type GraphData } from '$lib/graph/build';
import type { ChatInferenceProposal } from '$lib/api/client';

export function mergeEndorsedEdge(
	graph: GraphData,
	proposal: ChatInferenceProposal
): GraphData {
	const sourceNode = graph.nodes.find((n) => String(n.id) === String(proposal.source_concept_id));
	const targetNode = graph.nodes.find((n) => String(n.id) === String(proposal.target_concept_id));
	if (!sourceNode || !targetNode) {
		return graph;
	}
	const source = sourceNode.id;
	const target = targetNode.id;
	const label = proposal.proposed_type;
	const alreadyPresent = graph.links.some(
		(l) => l.source === source && l.target === target && l.label === label
	);
	if (alreadyPresent) {
		return graph;
	}
	return {
		nodes: graph.nodes,
		links: [
			...graph.links,
			{ source, target, label, color: EDGE_COLOR, asserted_by: [proposal.id] }
		]
	};
}
