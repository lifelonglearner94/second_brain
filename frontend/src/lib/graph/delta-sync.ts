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

export type FocusTarget = {
	addEventListener(type: string, listener: (event: Event) => void): void;
	removeEventListener(type: string, listener: (event: Event) => void): void;
};

export function onWindowFocus(target: FocusTarget, onFocus: () => void): () => void {
	const handler = (): void => onFocus();
	target.addEventListener('focus', handler);
	return () => target.removeEventListener('focus', handler);
}

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
