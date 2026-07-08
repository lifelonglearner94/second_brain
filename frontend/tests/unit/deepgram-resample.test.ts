import { describe, it, expect } from 'vitest';
import { resampleTo16kMono } from '../../src/lib/capture/deepgram';

describe('resampleTo16kMono — pure 16 kHz resampler (issue #82, iOS Safari sample-rate mismatch)', () => {
	it('is a no-op when the input is already 16 kHz (desktop/Android Chrome path)', () => {
		const input = new Float32Array([0, 0.25, 0.5, 0.75, 1]);
		expect(resampleTo16kMono(input, 16000)).toBe(input);
	});

	it('returns empty input unchanged', () => {
		const input = new Float32Array(0);
		expect(resampleTo16kMono(input, 48000)).toBe(input);
	});

	it('downsamples 48 kHz to 16 kHz (3:1) with the correct length', () => {
		const input = new Float32Array(1200);
		expect(resampleTo16kMono(input, 48000).length).toBe(400);
	});

	it('preserves a constant signal across resampling', () => {
		const input = new Float32Array(480).fill(0.4);
		const out = resampleTo16kMono(input, 48000);
		expect(out.length).toBe(160);
		for (const v of out) expect(v).toBeCloseTo(0.4, 6);
	});

	it('interpolates samples for a non-integer ratio (44.1 kHz -> 16 kHz)', () => {
		const input = new Float32Array(4410);
		for (let i = 0; i < input.length; i++) input[i] = i;
		const out = resampleTo16kMono(input, 44100);
		expect(out.length).toBe(1600);
		// Linear interpolation must stay within the source sample range and be
		// monotonically increasing for a ramp input.
		expect(out[0]).toBe(input[0]);
		expect(out[out.length - 1]).toBeLessThanOrEqual(input[input.length - 1]);
		for (let i = 1; i < out.length; i++) {
			expect(out[i]).toBeGreaterThanOrEqual(out[i - 1]);
		}
	});

	it('floors the output length when the ratio does not divide evenly', () => {
		// 16000/8000 * 7 = 14 exactly (upsample 2x); pick 8000 -> 16000 ratio 0.5.
		const input = new Float32Array([0, 1, 2, 3, 4, 5, 6]);
		const out = resampleTo16kMono(input, 8000);
		// ratio = 8000/16000 = 0.5 => outLen = floor(7 / 0.5) = 14
		expect(out.length).toBe(14);
	});
});
