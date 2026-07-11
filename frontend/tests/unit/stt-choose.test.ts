import { describe, it, expect, vi } from 'vitest';
import { chooseSttSource, describeSttAvailability } from '../../src/lib/capture/stt';
import type { SttSource } from '../../src/lib/capture/stt';

function fakeSource(label: 'deepgram' | 'web-speech'): SttSource {
	return {
		label,
		async start() {},
		async stop() {}
	};
}

describe('chooseSttSource - Deepgram primary when online, Web Speech offline fallback, keyboard-only otherwise', () => {
	it('picks Deepgram when online (backend proxy is reachable)', async () => {
		const buildDeepgram = vi.fn(() => fakeSource('deepgram'));
		const source = await chooseSttSource({
			online: true,
			buildDeepgram
		});
		expect(source?.label).toBe('deepgram');
		expect(buildDeepgram).toHaveBeenCalled();
	});

	it('picks Web Speech when offline but the browser exposes SpeechRecognition', async () => {
		const buildWebSpeech = vi.fn(() => fakeSource('web-speech'));
		const source = await chooseSttSource({
			online: false,
			webSpeechAvailable: true,
			buildWebSpeech
		});
		expect(source?.label).toBe('web-speech');
	});

	it('prefers Deepgram over Web Speech when online (Deepgram is the online primary)', async () => {
		const buildDeepgram = vi.fn(() => fakeSource('deepgram'));
		const buildWebSpeech = vi.fn(() => fakeSource('web-speech'));
		const source = await chooseSttSource({
			online: true,
			webSpeechAvailable: true,
			buildDeepgram,
			buildWebSpeech
		});
		expect(source?.label).toBe('deepgram');
		expect(buildWebSpeech).not.toHaveBeenCalled();
	});

	it('returns null when offline and Web Speech is unavailable (keyboard-only capture)', async () => {
		const source = await chooseSttSource({
			online: false,
			webSpeechAvailable: false
		});
		expect(source).toBeNull();
	});

	it('returns null when offline with no Web Speech fallback', async () => {
		const source = await chooseSttSource({ online: false });
		expect(source).toBeNull();
	});
});

describe('describeSttAvailability - graceful typing-only fallback when no STT source exists (issue #82)', () => {
	it('reports voice available when online (Deepgram proxy reachable)', () => {
		expect(describeSttAvailability({ online: true })).toEqual({
			canCaptureVoice: true,
			reason: null
		});
	});

	it('reports voice available when only Web Speech is available (offline but browser supports it)', () => {
		expect(describeSttAvailability({ online: false, webSpeechAvailable: true })).toEqual({
			canCaptureVoice: true,
			reason: null
		});
	});

	it('reports typing-only with a user-facing reason when offline and no Web Speech (iOS Safari offline)', () => {
		const avail = describeSttAvailability({ online: false, webSpeechAvailable: false });
		expect(avail.canCaptureVoice).toBe(false);
		expect(avail.reason).toMatch(/offline/i);
	});

	it('the reason is non-empty so the UI can surface it instead of silently failing', () => {
		const avail = describeSttAvailability({ online: false, webSpeechAvailable: false });
		expect(typeof avail.reason).toBe('string');
		expect((avail.reason ?? '').length).toBeGreaterThan(0);
	});
});