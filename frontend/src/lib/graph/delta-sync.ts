import type { GlobalTopologySnapshot, GraphDelta } from '$lib/api/client';
import { applyDelta } from './delta';

export type DeltaSyncState = {
	snapshot: GlobalTopologySnapshot;
	cursor: number;
};

export type DeltaSyncApi = {
	getGraphDelta(since: number): Promise<GraphDelta>;
};

export type DeltaSyncSuccess = {
	state: DeltaSyncState;
	applied: true;
	delta: GraphDelta;
};

export type DeltaSyncSkipped = {
	state: DeltaSyncState;
	applied: false;
};

export type DeltaSyncOutcome = DeltaSyncSuccess | DeltaSyncSkipped;

export async function syncDelta(state: DeltaSyncState, api: DeltaSyncApi): Promise<DeltaSyncOutcome> {
	try {
		const delta = await api.getGraphDelta(state.cursor);
		const snapshot = applyDelta(state.snapshot, delta);
		return {
			state: { snapshot, cursor: delta.cursor },
			applied: true,
			delta
		};
	} catch {
		return { state, applied: false };
	}
}
