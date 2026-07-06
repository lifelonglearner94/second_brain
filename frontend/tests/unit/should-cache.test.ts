import { describe, it, expect } from 'vitest';
import { shouldServeFromCache } from '../../src/lib/service-worker/should-cache';

const ORIGIN = 'http://localhost:4173';
const shell = new Set<string>([
	'/',
	'/_app/immutable/assets/app.css',
	'/favicon.png'
]);

function get(path: string, origin = ORIGIN): Request {
	return new Request(`${origin}${path}`, { method: 'GET' });
}

describe('shouldServeFromCache — the dumb Service Worker fetch gate (ADR-0005)', () => {
	it('serves a cached app-shell asset from the cache', () => {
		expect(shouldServeFromCache(get('/favicon.png'), shell, ORIGIN)).toBe(true);
		expect(shouldServeFromCache(get('/'), shell, ORIGIN)).toBe(true);
	});

	it('never intercepts API calls — the Edge /api/* proxy goes straight to the network', () => {
		expect(shouldServeFromCache(get('/api/health'), shell, ORIGIN)).toBe(false);
		expect(shouldServeFromCache(get('/api/ingest'), shell, ORIGIN)).toBe(false);
		expect(
			shouldServeFromCache(get('/api/graph/snapshot'), shell, ORIGIN)
		).toBe(false);
	});

	it('does not serve a non-GET request (POST braindump submit stays on the network)', () => {
		const post = new Request(`${ORIGIN}/api/ingest`, { method: 'POST' });
		expect(shouldServeFromCache(post, shell, ORIGIN)).toBe(false);
	});

	it('does not touch cross-origin requests (third-party stays off the SW)', () => {
		const cross = new Request('https://api.deepgram.com/v1/listen', {
			method: 'GET'
		});
		expect(shouldServeFromCache(cross, shell, ORIGIN)).toBe(false);
	});

	it('does not serve an app-shell path that is not in the precache', () => {
		expect(
			shouldServeFromCache(get('/some/uncached/page'), shell, ORIGIN)
		).toBe(false);
	});

	it('treats the bare app origin as an app-shell asset (the PWA entry document)', () => {
		expect(shouldServeFromCache(get('/'), shell, ORIGIN)).toBe(true);
	});
});
