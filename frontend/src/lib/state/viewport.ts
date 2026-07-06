export type ViewportState = {
	cameraX: number;
	cameraY: number;
	cameraZ: number;
	zoom: number;
	selectedNodeId: string | null;
};

const KEY = 'sb.viewport-state';

export function saveViewport(
	state: ViewportState,
	storage: Storage = globalThis.localStorage
): void {
	storage.setItem(KEY, JSON.stringify(state));
}

export function loadViewport(
	storage: Storage = globalThis.localStorage
): ViewportState | null {
	const raw = storage.getItem(KEY);
	if (!raw) return null;
	try {
		return JSON.parse(raw) as ViewportState;
	} catch {
		return null;
	}
}

export function clearViewport(
	storage: Storage = globalThis.localStorage
): void {
	storage.removeItem(KEY);
}
