import { describe, it, expect } from 'vitest';
import { AdminLogStore, adminLogs } from '../../src/lib/state/admin-logs.svelte';
import { SessionStore, session } from '../../src/lib/state/session.svelte';
import { PendingCapturesStore, pendingCaptures } from '../../src/lib/state/pending-captures.svelte';
import { EndorsementStore, endorsementQueue } from '../../src/lib/state/endorsement-queue.svelte';
import { HousekeepingStore, housekeeping } from '../../src/lib/state/housekeeping.svelte';
import { GraphStore, graphStore } from '../../src/lib/state/graph.svelte';

describe('store singletons — one shared instance exported from each store module (the spatial-graph pattern)', () => {
	it('adminLogs is the shared AdminLogStore singleton exported by the store module', () => {
		expect(adminLogs).toBeInstanceOf(AdminLogStore);
	});

	it('session is the shared SessionStore singleton exported by the store module', () => {
		expect(session).toBeInstanceOf(SessionStore);
	});

	it('pendingCaptures is the shared PendingCapturesStore singleton exported by the store module', () => {
		expect(pendingCaptures).toBeInstanceOf(PendingCapturesStore);
	});

	it('endorsementQueue is the shared EndorsementStore singleton exported by the store module', () => {
		expect(endorsementQueue).toBeInstanceOf(EndorsementStore);
	});

	it('housekeeping is the shared HousekeepingStore singleton exported by the store module', () => {
		expect(housekeeping).toBeInstanceOf(HousekeepingStore);
	});

	it('graphStore is the shared GraphStore singleton exported by the store module (the canonical Global Topology Snapshot holder)', () => {
		expect(graphStore).toBeInstanceOf(GraphStore);
	});

	it('each singleton is the same instance across re-imports (module-cached, not a fresh one per import)', async () => {
		const reloaded = await Promise.all([
			import('../../src/lib/state/admin-logs.svelte'),
			import('../../src/lib/state/session.svelte'),
			import('../../src/lib/state/pending-captures.svelte'),
			import('../../src/lib/state/endorsement-queue.svelte'),
			import('../../src/lib/state/housekeeping.svelte'),
			import('../../src/lib/state/graph.svelte')
		]);
		expect(reloaded[0].adminLogs).toBe(adminLogs);
		expect(reloaded[1].session).toBe(session);
		expect(reloaded[2].pendingCaptures).toBe(pendingCaptures);
		expect(reloaded[3].endorsementQueue).toBe(endorsementQueue);
		expect(reloaded[4].housekeeping).toBe(housekeeping);
		expect(reloaded[5].graphStore).toBe(graphStore);
	});
});
