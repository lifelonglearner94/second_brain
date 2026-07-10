import { describe, it, expect } from 'vitest';
import {
	saveViewport,
	loadViewport,
	clearViewport,
	type ViewportState
} from '../../src/lib/state/viewport';

class MemoryStorage implements Storage {
	private map = new Map<string, string>();
	get length() {
		return this.map.size;
	}
	clear() {
		this.map.clear();
	}
	getItem(key: string): string | null {
		return this.map.has(key) ? this.map.get(key)! : null;
	}
	key(index: number): string | null {
		return [...this.map.keys()][index] ?? null;
	}
	removeItem(key: string) {
		this.map.delete(key);
	}
	setItem(key: string, value: string) {
		this.map.set(key, value);
	}
}

const STATE: ViewportState = {
	cameraX: 1.5,
	cameraY: -2.25,
	cameraZ: 8,
	zoom: 1.1,
	selectedNodeId: 'concept-42'
};

describe('viewportState - LocalStorage home for observer state', () => {
	it('round-trips the viewport (camera, zoom, selected node) through LocalStorage', () => {
		const storage = new MemoryStorage();
		saveViewport(STATE, storage);
		expect(loadViewport(storage)).toEqual(STATE);
	});

	it('survives a JSON round-trip without mutating numeric fields', () => {
		const storage = new MemoryStorage();
		saveViewport(STATE, storage);
		const loaded = loadViewport(storage)!;
		expect(loaded.cameraX).toBeCloseTo(1.5);
		expect(loaded.zoom).toBeCloseTo(1.1);
	});

	it('returns null when no viewport has ever been saved (no amnesia-recovery data)', () => {
		const storage = new MemoryStorage();
		expect(loadViewport(storage)).toBeNull();
	});

	it('can be cleared so a reload starts from the default camera', () => {
		const storage = new MemoryStorage();
		saveViewport(STATE, storage);
		clearViewport(storage);
		expect(loadViewport(storage)).toBeNull();
	});

	it('tolerates a null selectedNodeId (no node selected)', () => {
		const storage = new MemoryStorage();
		const state: ViewportState = { ...STATE, selectedNodeId: null };
		saveViewport(state, storage);
		expect(loadViewport(storage)).toEqual(state);
	});

	it('returns null instead of throwing on corrupt storage', () => {
		const storage = new MemoryStorage();
		storage.setItem('sb.viewport-state', '{not json');
		expect(loadViewport(storage)).toBeNull();
	});
});
