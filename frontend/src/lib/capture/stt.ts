export type SttSourceLabel = 'deepgram' | 'web-speech';

export interface SttSource {
	readonly label: SttSourceLabel;
	start(onChunk: (chunk: string) => void): Promise<void>;
	stop(): Promise<void>;
}

export type SttSourceOptions = {
	online?: boolean;
	webSpeechAvailable?: boolean;
	buildDeepgram?: () => SttSource;
	buildWebSpeech?: () => SttSource;
};

/**
 * Whether any STT source is available to power voice capture, and a
 * user-facing reason when it is not. When no source is available (e.g. offline
 * with no Web Speech), the Active Capture must stay usable for typing and show
 * this reason rather than silently fail.
 */
export type SttAvailability = {
	canCaptureVoice: boolean;
	reason: string | null;
};

export function describeSttAvailability(opts: {
	online?: boolean;
	webSpeechAvailable?: boolean;
}): SttAvailability {
	if (opts.online || opts.webSpeechAvailable) {
		return { canCaptureVoice: true, reason: null };
	}
	return {
		canCaptureVoice: false,
		reason: 'Voice input unavailable offline - type your thought below.'
	};
}

export async function chooseSttSource(
	opts: SttSourceOptions
): Promise<SttSource | null> {
	// Deepgram is primary when online (backend proxy is reachable)
	if (opts.online) {
		if (opts.buildDeepgram) return opts.buildDeepgram();
		const { DeepgramSttSource } = await import('./deepgram');
		return new DeepgramSttSource();
	}
	// Web Speech is fallback when available
	if (opts.webSpeechAvailable) {
		if (opts.buildWebSpeech) return opts.buildWebSpeech();
		const { WebSpeechSttSource } = await import('./web-speech');
		return new WebSpeechSttSource();
	}
	return null;
}

export type SttSourcePair = {
	primary: SttSource | null;
	fallback: SttSource | null;
};

async function webSpeechSource(opts: SttSourceOptions): Promise<SttSource> {
	if (opts.buildWebSpeech) return opts.buildWebSpeech();
	const { WebSpeechSttSource } = await import('./web-speech');
	return new WebSpeechSttSource();
}

export async function buildSttSources(
	opts: SttSourceOptions
): Promise<SttSourcePair> {
	// Deepgram is primary when online, with Web Speech as fallback
	if (opts.online) {
		const primary = await chooseSttSource(opts);
		const fallback = opts.webSpeechAvailable
			? await webSpeechSource(opts)
			: null;
		return { primary, fallback };
	}
	// Web Speech only when offline but available
	if (opts.webSpeechAvailable) {
		return { primary: await webSpeechSource(opts), fallback: null };
	}
	return { primary: null, fallback: null };
}