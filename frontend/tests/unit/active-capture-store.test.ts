import { describe, it, expect } from 'vitest';
import { ActiveCaptureStore } from '../../src/lib/capture/active-capture.svelte';
import type { SttSource, SttSourceLabel } from '../../src/lib/capture/stt';
import type { IngestApi, IngestResponse } from '../../src/lib/capture/ingest';

function fakeSource(
	label: SttSourceLabel,
	opts: { emits?: string[]; fails?: string } = {}
): SttSource & { emit(chunk: string): void } {
	let onChunk: ((chunk: string) => void) | null = null;
	return {
		label,
		async start(cb: (chunk: string) => void) {
			onChunk = cb;
			if (opts.fails) throw new Error(opts.fails);
			for (const chunk of opts.emits ?? []) cb(chunk);
		},
		async stop() {
			onChunk = null;
		},
		emit(chunk: string) {
			onChunk?.(chunk);
		}
	};
}

function fakeIngest(res: IngestResponse): IngestApi & { calls: string[] } {
	const calls: string[] = [];
	return {
		async ingest(verbatim: string) {
			calls.push(verbatim);
			return res;
		},
		calls
	};
}

const INGESTED: IngestResponse = {
	braindump: { id: '7', created_at: '1790' },
	concepts: [{ id: 'c3', label: 'caffeine', created_at: '1790' }],
	edges: [],
	cursor: 1_800
};

describe('ActiveCaptureStore — the ephemeral, mutable text buffer (Active Capture, ADR-0007)', () => {
	it('starts idle with an empty buffer — nothing is a braindump until explicit submit', () => {
		const store = new ActiveCaptureStore();
		expect(store.text).toBe('');
		expect(store.status).toBe('idle');
		expect(store.sttSourceLabel).toBeNull();
	});

	it('accumulates streaming STT chunks into the buffer in real time (Deepgram Nova-3 → buffer)', () => {
		const store = new ActiveCaptureStore();
		store.appendSttChunk('hello ');
		store.appendSttChunk('world');
		expect(store.text).toBe('hello world');
	});

	it('manual keyboard input merges into the same buffer (setText edits the whole field, fixing STT hallucinations)', () => {
		const store = new ActiveCaptureStore();
		store.appendSttChunk('helo');
		store.setText('hello world');
		expect(store.text).toBe('hello world');
	});

	it('STT and keyboard share one buffer — a chunk appended after a keyboard edit carries on the same text', () => {
		const store = new ActiveCaptureStore();
		store.setText('caffeine');
		store.appendSttChunk(' disrupts sleep');
		expect(store.text).toBe('caffeine disrupts sleep');
	});

	it('clear() discards the buffer without submitting (the thought is abandoned, not ingested)', () => {
		const store = new ActiveCaptureStore();
		store.appendSttChunk('a thought');
		store.clear();
		expect(store.text).toBe('');
	});
});

describe('ActiveCaptureStore.startStt — streaming STT source feeds the buffer through the seam', () => {
	it('starts a Deepgram source, flips to listening, and feeds chunks into the buffer', async () => {
		const store = new ActiveCaptureStore();
		const source = fakeSource('deepgram', { emits: ['hallo', ' welt'] });
		await store.startStt(source);
		expect(store.status).toBe('listening');
		expect(store.sttSourceLabel).toBe('deepgram');
		expect(store.text).toBe('hallo welt');
	});

	it('a chunk emitted after start keeps feeding the buffer (real-time streaming)', async () => {
		const store = new ActiveCaptureStore();
		const source = fakeSource('deepgram');
		await store.startStt(source);
		source.emit('erstes ');
		source.emit('wort');
		expect(store.text).toBe('erstes wort');
	});

	it('stopStt() stops the source and returns to idle', async () => {
		const store = new ActiveCaptureStore();
		const source = fakeSource('deepgram');
		await store.startStt(source);
		await store.stopStt();
		expect(store.status).toBe('idle');
		expect(store.sttSourceLabel).toBeNull();
	});
});

describe('ActiveCaptureStore.startCaptureWithFallback — Deepgram primary, Web Speech offline fallback', () => {
	it('uses the Deepgram source when it connects and reports its label', async () => {
		const store = new ActiveCaptureStore();
		const deepgram = fakeSource('deepgram', { emits: ['online'] });
		const webSpeech = fakeSource('web-speech');
		const label = await store.startCaptureWithFallback(deepgram, webSpeech);
		expect(label).toBe('deepgram');
		expect(store.text).toBe('online');
	});

	it('falls back to Web Speech when Deepgram is unreachable (offline / WebSocket error)', async () => {
		const store = new ActiveCaptureStore();
		const deepgram = fakeSource('deepgram', { fails: 'deepgram unreachable' });
		const webSpeech = fakeSource('web-speech', { emits: ['offline'] });
		const label = await store.startCaptureWithFallback(deepgram, webSpeech);
		expect(label).toBe('web-speech');
		expect(store.sttSourceLabel).toBe('web-speech');
		expect(store.text).toBe('offline');
	});

	it('throws and flips to error when both Deepgram and Web Speech fail (no STT available)', async () => {
		const store = new ActiveCaptureStore();
		const deepgram = fakeSource('deepgram', { fails: 'deepgram unreachable' });
		const webSpeech = fakeSource('web-speech', { fails: 'web-speech unavailable' });
		await expect(store.startCaptureWithFallback(deepgram, webSpeech)).rejects.toThrow(/web-speech/);
		expect(store.status).toBe('error');
	});

	it('does not fall back when the primary succeeds (no redundant Web Speech session)', async () => {
		const store = new ActiveCaptureStore();
		const deepgram = fakeSource('deepgram', { emits: ['hi'] });
		const webSpeech = fakeSource('web-speech', { fails: 'should not be started' });
		const label = await store.startCaptureWithFallback(deepgram, webSpeech);
		expect(label).toBe('deepgram');
		expect(store.sttSourceLabel).toBe('deepgram');
	});
});

describe('ActiveCaptureStore.submit — explicit-submit gate (ADR-0007: nothing is a braindump until submit)', () => {
	it('POSTs the verbatim only on explicit submit, not while STT chunks are arriving', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		const source = fakeSource('deepgram');
		await store.startStt(source);
		source.emit('caffeine ');
		source.emit('disrupts sleep');
		expect(ingest.calls).toHaveLength(0);
		await store.submit(ingest);
		expect(ingest.calls).toEqual(['caffeine disrupts sleep']);
	});

	it('POSTs the verbatim assembled from STT + keyboard merges', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		store.appendSttChunk('caffein disrupts');
		store.setText('caffeine disrupts sleep');
		await store.submit(ingest);
		expect(ingest.calls).toEqual(['caffeine disrupts sleep']);
	});

	it('discards the buffer after a successful submit (the Active Capture is ephemeral)', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		store.setText('a thought');
		await store.submit(ingest);
		expect(store.text).toBe('');
		expect(store.status).toBe('submitted');
		expect(store.sttSourceLabel).toBeNull();
	});

	it('returns the ingested concepts/edges so the caller can optimistically merge them', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		store.setText('caffeine disrupts sleep');
		const res = await store.submit(ingest);
		expect(res.concepts[0]?.label).toBe('caffeine');
		expect(res.cursor).toBe(1_800);
	});

	it('refuses to submit an empty buffer (no empty braindumps — backend #5 rejects empty verbatim)', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		await expect(store.submit(ingest)).rejects.toThrow(/empty/i);
		expect(ingest.calls).toHaveLength(0);
	});

	it('refuses to submit a whitespace-only buffer', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		store.setText('   \n\t  ');
		await expect(store.submit(ingest)).rejects.toThrow(/empty/i);
		expect(ingest.calls).toHaveLength(0);
	});

	it('stops the STT source on submit (the mic/WebSocket closes with the buffer)', async () => {
		const store = new ActiveCaptureStore();
		const ingest = fakeIngest(INGESTED);
		const source = fakeSource('deepgram');
		await store.startStt(source);
		source.emit('a thought');
		await store.submit(ingest);
		expect(store.sttSourceLabel).toBeNull();
	});

	it('flips to error and keeps the buffer when the ingest fails (so the user can retry / edit)', async () => {
		const store = new ActiveCaptureStore();
		const ingest: IngestApi = {
			async ingest() {
				throw new Error('POST /braindumps failed: 503');
			}
		};
		store.setText('a thought');
		await expect(store.submit(ingest)).rejects.toThrow(/503/);
		expect(store.status).toBe('error');
		expect(store.text).toBe('a thought');
	});
});
