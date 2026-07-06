export type RendererChoice = '3d' | '2d';

export type CapabilityEnv = {
	userAgent: string;
	hasWebGL2: boolean;
	webglRenderer: string | null;
	hardwareConcurrency: number;
};

const WEAK_MOBILE_GPU =
	/(adreno\s*[0-6]\d{2}|mali|powervr|apple gpu|powervr sgx)/i;
const IOS_PATTERN = /(iphone|ipad|ipod)/i;
const ANDROID_PATTERN = /android/i;

export function detectRendererCapability(env: CapabilityEnv): RendererChoice {
	if (!env.hasWebGL2) {
		return '2d';
	}
	const ua = env.userAgent;
	if (IOS_PATTERN.test(ua)) {
		return '2d';
	}
	if (ANDROID_PATTERN.test(ua)) {
		if (env.hardwareConcurrency < 6) {
			return '2d';
		}
		const renderer = env.webglRenderer ?? '';
		if (WEAK_MOBILE_GPU.test(renderer)) {
			return '2d';
		}
	}
	return '3d';
}

export function probeRendererCapability(): CapabilityEnv {
	let hasWebGL2 = false;
	let webglRenderer: string | null = null;
	try {
		const canvas = document.createElement('canvas');
		const gl = canvas.getContext('webgl2') as WebGL2RenderingContext | null;
		if (gl) {
			hasWebGL2 = true;
			const ext = gl.getExtension('WEBGL_debug_renderer_info');
			if (ext) {
				webglRenderer = gl.getParameter(ext.UNMASKED_RENDERER_WEBGL);
			}
		}
	} catch {
		/* noop */
	}
	return {
		userAgent: typeof navigator !== 'undefined' ? navigator.userAgent : '',
		hasWebGL2,
		webglRenderer,
		hardwareConcurrency:
			typeof navigator !== 'undefined' &&
			typeof navigator.hardwareConcurrency === 'number'
				? navigator.hardwareConcurrency
				: 4
	};
}
