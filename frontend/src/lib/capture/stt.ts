export type SttSourceLabel = 'deepgram' | 'web-speech';

export interface SttSource {
	readonly label: SttSourceLabel;
	start(onChunk: (chunk: string) => void): Promise<void>;
	stop(): Promise<void>;
}

export type SttSourceOptions = {
	deepgramApiKey?: string;
	webSpeechAvailable?: boolean;
	buildDeepgram?: (apiKey: string) => SttSource;
	buildWebSpeech?: () => SttSource;
};

export async function chooseSttSource(opts: SttSourceOptions): Promise<SttSource | null> {
	if (opts.deepgramApiKey) {
		if (opts.buildDeepgram) return opts.buildDeepgram(opts.deepgramApiKey);
		const { DeepgramSttSource } = await import('./deepgram');
		return new DeepgramSttSource({ apiKey: opts.deepgramApiKey });
	}
	if (opts.webSpeechAvailable) {
		if (opts.buildWebSpeech) return opts.buildWebSpeech();
		const { WebSpeechSttSource } = await import('./web-speech');
		return new WebSpeechSttSource();
	}
	return null;
}
