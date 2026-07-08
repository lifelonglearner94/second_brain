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

describe('chooseSttSource — Deepgram primary, Web Speech offline fallback, keyboard-only otherwise', () => {
	it('picks Deepgram (Nova-3) when an API key is configured', async () => {
		const buildDeepgram = vi.fn((key: string) => {
			void key;
			return fakeSource('deepgram');
		});
		const source = await chooseSttSource({
			deepgramApiKey: 'dg-key',
			buildDeepgram
		});
		expect(source?.label).toBe('deepgram');
		expect(buildDeepgram).toHaveBeenCalledWith('dg-key');
	});

	it('picks Web Speech when Deepgram is not configured but the browser exposes SpeechRecognition', async () => {
		const buildWebSpeech = vi.fn(() => fakeSource('web-speech'));
		const source = await chooseSttSource({
			webSpeechAvailable: true,
			buildWebSpeech
		});
		expect(source?.label).toBe('web-speech');
	});

	it('prefers Deepgram over Web Speech when both are available (Deepgram is the online primary)', async () => {
		const buildDeepgram = vi.fn(() => fakeSource('deepgram'));
		const buildWebSpeech = vi.fn(() => fakeSource('web-speech'));
		const source = await chooseSttSource({
			deepgramApiKey: 'dg-key',
			webSpeechAvailable: true,
			buildDeepgram,
			buildWebSpeech
		});
		expect(source?.label).toBe('deepgram');
		expect(buildWebSpeech).not.toHaveBeenCalled();
	});

	it('returns null when neither Deepgram nor Web Speech is available (keyboard-only capture)', async () => {
		const source = await chooseSttSource({});
		expect(source).toBeNull();
	});

	it('returns null when Web Speech is unavailable and Deepgram is not configured (offline, no fallback)', async () => {
		const source = await chooseSttSource({ webSpeechAvailable: false });
		expect(source).toBeNull();
	});
});

describe('describeSttAvailability — graceful typing-only fallback when no STT source exists (issue #82)', () => {
	it('reports voice available with no reason when a Deepgram key is configured', () => {
		expect(describeSttAvailability({ deepgramApiKey: 'k' })).toEqual({
			canCaptureVoice: true,
			reason: null
		});
	});

	it('reports voice available when only Web Speech is available (no Deepgram key)', () => {
		expect(describeSttAvailability({ webSpeechAvailable: true })).toEqual({
			canCaptureVoice: true,
			reason: null
		});
	});

	it('reports typing-only with a user-facing reason when neither source is available (iOS Safari, no key, no Web Speech)', () => {
		const avail = describeSttAvailability({});
		expect(avail.canCaptureVoice).toBe(false);
		expect(avail.reason).toMatch(/type your thought/i);
	});

	it('the reason is non-empty so the UI can surface it instead of silently failing', () => {
		const avail = describeSttAvailability({ webSpeechAvailable: false });
		expect(typeof avail.reason).toBe('string');
		expect((avail.reason ?? '').length).toBeGreaterThan(0);
	});
});
