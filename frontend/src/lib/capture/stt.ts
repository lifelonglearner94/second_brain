export type SttSourceLabel = 'deepgram' | 'web-speech';

export interface SttSource {
	readonly label: SttSourceLabel;
	start(onChunk: (chunk: string) => void): Promise<void>;
	stop(): Promise<void>;
}
