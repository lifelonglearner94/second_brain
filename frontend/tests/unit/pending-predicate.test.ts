import { describe, it, expect } from 'vitest';
import { shouldQueuePending } from '../../src/lib/capture/pending';
import type { SttSourceLabel } from '../../src/lib/capture/stt';

describe('shouldQueuePending — the offline write-intent routing predicate (ADR-0005/0007)', () => {
	it('routes to Pending Captures when the browser is offline, regardless of STT source', () => {
		expect(shouldQueuePending(false, 'deepgram')).toBe(true);
		expect(shouldQueuePending(false, 'web-speech')).toBe(true);
		expect(shouldQueuePending(false, null)).toBe(true);
	});

	it('routes to Pending Captures when only the offline STT fallback filled the buffer (web-speech), even online', () => {
		expect(shouldQueuePending(true, 'web-speech' as SttSourceLabel)).toBe(true);
	});

	it('submits immediately through the #19 ingest path when online with Deepgram', () => {
		expect(shouldQueuePending(true, 'deepgram')).toBe(false);
	});

	it('submits immediately when online with no STT label (e.g. pure keyboard input)', () => {
		expect(shouldQueuePending(true, null)).toBe(false);
	});
});
