import { describe, it, expect, vi } from 'vitest';
import {
	SessionStore,
	type SessionApi
} from '../../src/lib/state/session.svelte';
import type { Me } from '../../src/lib/api/client';

const ME: Me = { user_id: '00000000-0000-0000-0000-000000000001' };

function apiStub(getMe: SessionApi['getMe']): SessionApi {
	return { getMe };
}

describe('SessionStore — opaque-cookie auth state (reload stays authenticated)', () => {
	it('starts unknown so the guard can decide after the first /me probe', () => {
		const store = new SessionStore(apiStub(vi.fn<SessionApi['getMe']>()));
		expect(store.status).toBe('unknown');
		expect(store.userId).toBeNull();
	});

	it('refresh() flips to authenticated when GET /me returns the account id', async () => {
		const getMe = vi.fn<SessionApi['getMe']>();
		getMe.mockResolvedValue(ME);
		const store = new SessionStore(apiStub(getMe));
		await store.refresh();
		expect(store.status).toBe('authenticated');
		expect(store.userId).toBe('00000000-0000-0000-0000-000000000001');
	});

	it('refresh() flips to unauthenticated when GET /me rejects (401, no session)', async () => {
		const getMe = vi.fn<SessionApi['getMe']>();
		getMe.mockRejectedValue(new Error('GET /me failed: 401'));
		const store = new SessionStore(apiStub(getMe));
		await store.refresh();
		expect(store.status).toBe('unauthenticated');
		expect(store.userId).toBeNull();
	});

	it('setAuthenticated() marks the session live after login without a second /me round-trip', () => {
		const store = new SessionStore(apiStub(vi.fn<SessionApi['getMe']>()));
		store.setAuthenticated('00000000-0000-0000-0000-000000000001');
		expect(store.status).toBe('authenticated');
		expect(store.userId).toBe('00000000-0000-0000-0000-000000000001');
	});

	it('clear() marks the session gone after logout', () => {
		const store = new SessionStore(apiStub(vi.fn<SessionApi['getMe']>()));
		store.setAuthenticated('00000000-0000-0000-0000-000000000001');
		store.clear();
		expect(store.status).toBe('unauthenticated');
		expect(store.userId).toBeNull();
	});

	it('refresh() recovers from unauthenticated back to authenticated on reconnect', async () => {
		let ok = false;
		const getMe = vi.fn<SessionApi['getMe']>(async () => {
			if (!ok) throw new Error('GET /me failed: 401');
			return ME;
		});
		const store = new SessionStore(apiStub(getMe));
		await store.refresh();
		expect(store.status).toBe('unauthenticated');
		ok = true;
		await store.refresh();
		expect(store.status).toBe('authenticated');
	});
});
