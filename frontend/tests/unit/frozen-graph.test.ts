import { describe, it, expect } from 'vitest';
import { frozenGraphStatus } from '../../src/lib/graph/frozen-graph';

describe('frozenGraphStatus — the Frozen Graph staleness indicator contract (ADR-0005, issue #21)', () => {
	it('returns offline + a label showing the fetchedAt timestamp and the offline state when the snapshot came from cache', () => {
		const outcome = frozenGraphStatus('cache', '2026-07-04T12:00:00Z', true);
		expect(outcome.status).toBe('offline');
		expect(outcome.label).not.toBeNull();
		expect(outcome.label).toContain('2026-07-04T12:00:00Z');
		expect(outcome.label?.toLowerCase()).toContain('offline');
	});

	it('returns ready + no label when the snapshot came from the network while online', () => {
		const outcome = frozenGraphStatus('network', '2026-07-04T12:00:00Z', true);
		expect(outcome.status).toBe('ready');
		expect(outcome.label).toBeNull();
	});

	it('returns offline when the browser reports offline even if a network snapshot raced through', () => {
		const outcome = frozenGraphStatus('network', '2026-07-04T12:00:00Z', false);
		expect(outcome.status).toBe('offline');
		expect(outcome.label?.toLowerCase()).toContain('offline');
		expect(outcome.label).toContain('2026-07-04T12:00:00Z');
	});

	it('never yields a blank screen on connectivity failure — the error case maps to a non-null label carrying the error message', () => {
		const outcome = frozenGraphStatus('error', null, false, 'Global Topology Snapshot unavailable: backend unreachable and no cached snapshot');
		expect(outcome.status).toBe('error');
		expect(outcome.label).not.toBeNull();
		expect(outcome.label).toContain('Could not load the graph');
		expect(outcome.label).toContain('unavailable');
	});

	it('falls back to "unknown" in the staleness label when the cached snapshot has no fetchedAt timestamp (still non-blank)', () => {
		const outcome = frozenGraphStatus('cache', null, false);
		expect(outcome.status).toBe('offline');
		expect(outcome.label).not.toBeNull();
		expect(outcome.label).toContain('unknown');
	});
});
