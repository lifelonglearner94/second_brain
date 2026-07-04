import type { SttSource } from './stt';

export type WebSpeechSttSourceOptions = {
	lang?: string;
};

export class WebSpeechSttSource implements SttSource {
	readonly label = 'web-speech' as const;

	private recognition: SpeechRecognition | null = null;
	private onChunk: ((chunk: string) => void) | null = null;

	constructor(private opts: WebSpeechSttSourceOptions = {}) {}

	async start(onChunk: (chunk: string) => void): Promise<void> {
		const ctor = window.SpeechRecognition ?? window.webkitSpeechRecognition;
		if (!ctor) {
			throw new Error('Web Speech API unavailable in this browser');
		}
		this.onChunk = onChunk;
		const recognition = new ctor();
		recognition.lang = this.opts.lang ?? 'de-DE';
		recognition.continuous = true;
		recognition.interimResults = true;
		recognition.maxAlternatives = 1;

		const started = new Promise<void>((resolve, reject) => {
			recognition.onstart = () => resolve();
			recognition.onerror = (e: SpeechRecognitionErrorEvent) =>
				reject(new Error(`Web Speech error: ${e.error}`));
		});

		recognition.onresult = (event: SpeechRecognitionEvent) => {
			if (!this.onChunk) return;
			for (let i = event.resultIndex; i < event.results.length; i++) {
				const result = event.results[i];
				if (result.isFinal) {
					const transcript = result[0]?.transcript ?? '';
					if (transcript) this.onChunk(transcript + ' ');
				}
			}
		};

		this.recognition = recognition;
		recognition.start();
		await started;
	}

	async stop(): Promise<void> {
		const recognition = this.recognition;
		this.recognition = null;
		this.onChunk = null;
		if (recognition) {
			try {
				recognition.stop();
			} catch {
				/* already stopped */
			}
		}
	}
}
