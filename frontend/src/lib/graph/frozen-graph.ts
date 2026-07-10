import type { SnapshotSource } from './load';

export type FrozenGraphSource = SnapshotSource | 'error';

export type FrozenGraphOutcome = {
	status: 'ready' | 'offline' | 'error';
	label: string | null;
};

export function frozenGraphStatus(
	source: FrozenGraphSource,
	fetchedAt: string | null,
	online: boolean,
	error: string | null = null
): FrozenGraphOutcome {
	if (source === 'error') {
		return {
			status: 'error',
			label: `Could not load the graph: ${error ?? 'unknown error'}`
		};
	}
	if (source === 'cache' || !online) {
		const ts = fetchedAt ?? 'unknown';
		return {
			status: 'offline',
			label: `Offline - showing graph as of ${ts} (Frozen Graph).`
		};
	}
	return { status: 'ready', label: null };
}
