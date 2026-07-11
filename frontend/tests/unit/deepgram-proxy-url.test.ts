import { describe, it, expect } from 'vitest';
import { buildProxyUrl } from '../../src/lib/capture/deepgram';

describe('buildProxyUrl', () => {
	it('uses /api prefix by default (Caddy strips it → backend /stt/deepgram)', () => {
		expect(
			buildProxyUrl({
				location: { protocol: 'https:', host: 'brain.example.com' }
			})
		).toBe('wss://brain.example.com/api/stt/deepgram');
	});

	it('uses ws: when the page is served over http (local dev)', () => {
		expect(
			buildProxyUrl({
				location: { protocol: 'http:', host: 'localhost:5173' }
			})
		).toBe('ws://localhost:5173/api/stt/deepgram');
	});

	it('connects directly to the backend when VITE_BACKEND_BASE_URL is set (no /api prefix)', () => {
		expect(
			buildProxyUrl({
				backendBase: 'http://127.0.0.1:8080',
				location: { protocol: 'http:', host: 'localhost:5173' }
			})
		).toBe('ws://127.0.0.1:8080/stt/deepgram');
	});

	it('uses wss: when the backend base URL is https', () => {
		expect(
			buildProxyUrl({
				backendBase: 'https://api.example.com',
				location: { protocol: 'https:', host: 'brain.example.com' }
			})
		).toBe('wss://api.example.com/stt/deepgram');
	});

	it('preserves a non-root path in the backend base URL', () => {
		expect(
			buildProxyUrl({
				backendBase: 'http://127.0.0.1:8080/backend',
				location: { protocol: 'http:', host: 'localhost:5173' }
			})
		).toBe('ws://127.0.0.1:8080/backend/stt/deepgram');
	});

	it('does not double-slash when the base path ends with /', () => {
		expect(
			buildProxyUrl({
				backendBase: 'http://127.0.0.1:8080/',
				location: { protocol: 'http:', host: 'localhost:5173' }
			})
		).toBe('ws://127.0.0.1:8080/stt/deepgram');
	});

	it('defaults to /api when backendBase is undefined (same as the API client)', () => {
		expect(
			buildProxyUrl({
				backendBase: undefined,
				location: { protocol: 'https:', host: 'brain.example.com' }
			})
		).toBe('wss://brain.example.com/api/stt/deepgram');
	});
});
