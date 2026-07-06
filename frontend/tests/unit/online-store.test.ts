import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { OnlineStore } from '../../src/lib/state/online.svelte';

const originalOnLine = Object.getOwnPropertyDescriptor(navigator, 'onLine');

function setOnLine(value: boolean): void {
	Object.defineProperty(navigator, 'onLine', {
		value,
		configurable: true,
		writable: true
	});
}

afterEach(() => {
	if (originalOnLine) {
		Object.defineProperty(navigator, 'onLine', originalOnLine);
	}
	vi.unstubAllGlobals();
	window.dispatchEvent(new Event('online'));
});

describe('OnlineStore — shared browser connectivity state for the offline read-only mode (ADR-0005, issue #21)', () => {
	it('defaults to navigator.onLine at construction time (true in jsdom)', () => {
		setOnLine(true);
		const store = new OnlineStore();
		expect(store.online).toBe(true);
	});

	it('defaults to false when the browser reports offline at construction time', () => {
		setOnLine(false);
		const store = new OnlineStore();
		expect(store.online).toBe(false);
	});

	it('defaults to online when navigator is unavailable (SSR guard)', () => {
		vi.stubGlobal('navigator', undefined);
		const store = new OnlineStore();
		expect(store.online).toBe(true);
	});

	describe('init() — window online/offline listeners', () => {
		beforeEach(() => {
			setOnLine(true);
		});

		it("flips to offline when the window dispatches the 'offline' event", () => {
			const store = new OnlineStore();
			store.init();
			expect(store.online).toBe(true);
			setOnLine(false);
			window.dispatchEvent(new Event('offline'));
			expect(store.online).toBe(false);
		});

		it("recovers to online when the window dispatches the 'online' event", () => {
			setOnLine(false);
			const store = new OnlineStore();
			store.init();
			setOnLine(true);
			window.dispatchEvent(new Event('online'));
			expect(store.online).toBe(true);
		});

		it('returns a cleanup that removes both listeners so later events no longer mutate state', () => {
			const store = new OnlineStore();
			const cleanup = store.init();
			cleanup();
			setOnLine(false);
			window.dispatchEvent(new Event('offline'));
			expect(store.online).toBe(true);
			setOnLine(true);
			window.dispatchEvent(new Event('online'));
			expect(store.online).toBe(true);
		});
	});
});
