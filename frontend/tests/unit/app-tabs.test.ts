import { describe, it, expect } from 'vitest';
import { APP_TABS, isTabActive } from '../../src/lib/state/app-tabs';

describe('isTabActive — route-based tab highlight (issue #56)', () => {
	it('marks /app active only on the Capture route, not on deeper /app/* routes', () => {
		expect(isTabActive('/app', '/app')).toBe(true);
		expect(isTabActive('/app', '/app/graph')).toBe(false);
		expect(isTabActive('/app', '/app/chat')).toBe(false);
		expect(isTabActive('/app', '/app/pending')).toBe(false);
		expect(isTabActive('/app', '/app/admin/logs')).toBe(false);
	});

	it('marks /app/graph active only on the Graph route', () => {
		expect(isTabActive('/app/graph', '/app/graph')).toBe(true);
		expect(isTabActive('/app/graph', '/app')).toBe(false);
		expect(isTabActive('/app/graph', '/app/chat')).toBe(false);
		expect(isTabActive('/app/graph', '/app/graph/deep')).toBe(true);
	});

	it('marks /app/chat active on the Chat route and its subpaths', () => {
		expect(isTabActive('/app/chat', '/app/chat')).toBe(true);
		expect(isTabActive('/app/chat', '/app/chat/something')).toBe(true);
		expect(isTabActive('/app/chat', '/app')).toBe(false);
		expect(isTabActive('/app/chat', '/app/graph')).toBe(false);
	});

	it('exposes exactly the three canonical tabs in stable order', () => {
		expect(APP_TABS.map((t) => t.href)).toEqual(['/app', '/app/graph', '/app/chat']);
		expect(APP_TABS.map((t) => t.label)).toEqual(['Capture', 'Graph', 'Chat']);
	});
});
