import { mergeEndorsedEdge } from '$lib/endorsement/merge';
import { buildGraphData, type GraphData } from '$lib/graph/build';
import type { ChatInferenceProposal, GlobalTopologySnapshot } from '$lib/api/client';

export type SpatialGraphStatus = 'idle' | 'loading' | 'loaded' | 'error';

export class SpatialGraphStore {
	data = $state<GraphData>({ nodes: [], links: [] });
	status = $state<SpatialGraphStatus>('idle');
	error = $state<string | null>(null);

	setData(data: GraphData): void {
		this.data = data;
	}

	loadSnapshot(snapshot: GlobalTopologySnapshot): void {
		this.data = buildGraphData(snapshot);
	}

	mergeEndorsedEdge(proposal: ChatInferenceProposal): void {
		this.data = mergeEndorsedEdge(this.data, proposal);
	}
}

export const spatialGraph = new SpatialGraphStore();
