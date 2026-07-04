import type { GlobalTopologySnapshot } from '$lib/api/client';
import type { IdbStore, TopologySnapshot } from '$lib/state/idb';

export type SnapshotSource = 'network' | 'cache';

export type LoadedSnapshot = {
	snapshot: TopologySnapshot;
	source: SnapshotSource;
};

export type SnapshotApi = {
	getGraph(): Promise<GlobalTopologySnapshot>;
};

export async function loadSpatialViewGraph(
	api: SnapshotApi,
	idb: IdbStore,
	now: () => string = (): string => new Date().toISOString()
): Promise<LoadedSnapshot> {
	try {
		const raw = await api.getGraph();
		const snapshot: TopologySnapshot = { ...raw, fetchedAt: now() };
		await idb.saveTopologySnapshot(snapshot);
		return { snapshot, source: 'network' };
	} catch {
		const cached = await idb.loadTopologySnapshot();
		if (cached) {
			return { snapshot: cached, source: 'cache' };
		}
		throw new Error('Global Topology Snapshot unavailable: backend unreachable and no cached snapshot');
	}
}
