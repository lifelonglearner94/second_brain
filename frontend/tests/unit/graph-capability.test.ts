import { describe, it, expect } from 'vitest';
import {
	detectRendererCapability,
	type CapabilityEnv
} from '../../src/lib/graph/capability';

const DESKTOP_CHROME: CapabilityEnv = {
	userAgent:
		'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36',
	hasWebGL2: true,
	webglRenderer:
		'ANGLE (NVIDIA, NVIDIA GeForce RTX 3060 Direct3D11 vs_5_0 ps_5_0)',
	hardwareConcurrency: 8
};

const ANDROID_MIDRANGE_ADRENO: CapabilityEnv = {
	userAgent:
		'Mozilla/5.0 (Linux; Android 12; SM-A325F) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Mobile Safari/537.36',
	hasWebGL2: true,
	webglRenderer: 'Adreno 618',
	hardwareConcurrency: 8
};

const IPHONE: CapabilityEnv = {
	userAgent:
		'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1',
	hasWebGL2: true,
	webglRenderer: 'Apple GPU',
	hardwareConcurrency: 6
};

const NO_WEBGL2: CapabilityEnv = {
	userAgent: DESKTOP_CHROME.userAgent,
	hasWebGL2: false,
	webglRenderer: null,
	hardwareConcurrency: 8
};

describe('detectRendererCapability — picks 3D vs 2D for the Spatial View-Graph', () => {
	it('chooses 3D on a capable desktop with WebGL2 and a discrete GPU', () => {
		expect(detectRendererCapability(DESKTOP_CHROME)).toBe('3d');
	});

	it('falls back to 2D when WebGL2 is unavailable (3d-force-graph cannot run)', () => {
		expect(detectRendererCapability(NO_WEBGL2)).toBe('2d');
	});

	it('falls back to 2D on iOS (3d-force-graph perf is poor on Safari)', () => {
		expect(detectRendererCapability(IPHONE)).toBe('2d');
	});

	it('falls back to 2D on a mid-range Android with a weak mobile GPU', () => {
		expect(detectRendererCapability(ANDROID_MIDRANGE_ADRENO)).toBe('2d');
	});

	it('chooses 3D on a high-end Android with a recent Adreno and 8 cores', () => {
		const highEndAndroid: CapabilityEnv = {
			userAgent:
				'Mozilla/5.0 (Linux; Android 14; SM-S928B) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Mobile Safari/537.36',
			hasWebGL2: true,
			webglRenderer: 'Adreno 750',
			hardwareConcurrency: 8
		};
		expect(detectRendererCapability(highEndAndroid)).toBe('3d');
	});

	it('falls back to 2D on Android when hardware concurrency is low regardless of GPU string', () => {
		const weakAndroid: CapabilityEnv = {
			userAgent: ANDROID_MIDRANGE_ADRENO.userAgent,
			hasWebGL2: true,
			webglRenderer: 'Adreno 750',
			hardwareConcurrency: 4
		};
		expect(detectRendererCapability(weakAndroid)).toBe('2d');
	});
});
