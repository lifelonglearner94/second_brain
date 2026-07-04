import { mergeEndorsedEdge } from '$lib/endorsement/merge';
import { buildGraphData, type GraphData } from '$lib/graph/build';
import { applyDelta } from '$lib/graph/delta';
import { applyConceptMerge, applyTypeMerge } from '$lib/graph/merge';
import { syncDelta, type DeltaSyncApi } from '$lib/graph/delta-sync';
import { loadSpatialViewGraph, type SnapshotApi, type SnapshotSource } from '$lib/graph/load';
import type { IdbStore, TopologySnapshot } from '$lib/state/idb';
import type {
	ChatInferenceProposal,
	ConceptMergeSuggestion,
	GlobalTopologySnapshot,
	GraphDelta,
	OntologyTypeProposal
} from '$lib/api/client';
import type { IngestResponse } from '$lib/capture/ingest';

export type GraphStoreStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type SyncDeltaOutcome = {
	applied: boolean;
	delta?: GraphDelta;
};

export class GraphStore {
	snapshot = $state<TopologySnapshot | null>(null);
	cursor = $state(0);
	data = $state<GraphData>({ nodes: [], links: [] });
	status = $state<GraphStoreStatus>('idle');
	error = $state<string | null>(null);
	source = $state<SnapshotSource | null>(null);
	fetchedAt = $state<string | null>(null);

	async loadFromNetworkOrCache(
		api: SnapshotApi,
		idb: IdbStore,
		now: () => string = (): string => new Date().toISOString()
	): Promise<{ source: SnapshotSource; fetchedAt: string }> {
		if (this.snapshot) {
			return { source: this.source ?? 'network', fetchedAt: this.fetchedAt ?? '' };
		}
		this.status = 'loading';
		this.error = null;
		const loaded = await loadSpatialViewGraph(api, idb, now);
		this.snapshot = loaded.snapshot;
		this.fetchedAt = loaded.snapshot.fetchedAt;
		this.source = loaded.source;
		this.cursor = Math.floor(new Date(loaded.snapshot.fetchedAt).getTime() / 1000);
		this.data = buildGraphData(loaded.snapshot);
		this.status = 'loaded';
		return { source: loaded.source, fetchedAt: loaded.snapshot.fetchedAt };
	}

	async syncDelta(api: DeltaSyncApi): Promise<SyncDeltaOutcome> {
		if (!this.snapshot) return { applied: false };
		const outcome = await syncDelta({ snapshot: this.snapshot, cursor: this.cursor }, api);
		if (!outcome.applied) return { applied: false };
		this.snapshot = stampFetchedAt(outcome.state.snapshot, this.fetchedAt);
		this.cursor = outcome.state.cursor;
		if (hasDeltaChanges(outcome.delta)) {
			this.data = buildGraphData(this.snapshot);
		}
		return { applied: true, delta: outcome.delta };
	}

	mergeIngest(res: IngestResponse): void {
		if (!this.snapshot) return;
		this.snapshot = stampFetchedAt(
			applyDelta(this.snapshot, {
				cursor: res.cursor,
				added_concepts: res.concepts,
				added_edges: res.edges,
				deleted_concept_ids: [],
				deleted_edge_ids: [],
				retagged_edges: []
			}),
			this.fetchedAt
		);
		this.cursor = res.cursor;
		this.data = buildGraphData(this.snapshot);
	}

	mergeEndorsedEdge(proposal: ChatInferenceProposal): void {
		this.data = mergeEndorsedEdge(this.data, proposal);
	}

	applyConceptMerge(suggestion: ConceptMergeSuggestion): GlobalTopologySnapshot | null {
		if (!this.snapshot) return null;
		this.snapshot = stampFetchedAt(applyConceptMerge(this.snapshot, suggestion), this.fetchedAt);
		this.data = buildGraphData(this.snapshot);
		return this.snapshot;
	}

	applyTypeMerge(proposal: OntologyTypeProposal): GlobalTopologySnapshot | null {
		if (!this.snapshot) return null;
		this.snapshot = stampFetchedAt(applyTypeMerge(this.snapshot, proposal), this.fetchedAt);
		this.data = buildGraphData(this.snapshot);
		return this.snapshot;
	}

	loadSnapshot(snapshot: GlobalTopologySnapshot): void {
		this.snapshot = { ...snapshot, fetchedAt: this.fetchedAt ?? new Date(0).toISOString() };
		this.data = buildGraphData(snapshot);
		this.status = 'loaded';
	}
}

function stampFetchedAt(snapshot: GlobalTopologySnapshot, fetchedAt: string | null): TopologySnapshot {
	return { ...snapshot, fetchedAt: fetchedAt ?? new Date(0).toISOString() };
}

function hasDeltaChanges(delta: GraphDelta): boolean {
	return (
		delta.added_concepts.length > 0 ||
		delta.added_edges.length > 0 ||
		delta.deleted_concept_ids.length > 0 ||
		delta.deleted_edge_ids.length > 0 ||
		delta.retagged_edges.length > 0
	);
}

export const graphStore = new GraphStore();
