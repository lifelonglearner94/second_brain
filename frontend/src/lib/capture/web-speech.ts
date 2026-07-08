import type { SttSource } from './stt';

export type WebSpeechSttSourceOptions = {
	lang?: string;
};

/**
 * Minimal, DOM-free view of one Web Speech recognition result. A result is
 * `isFinal` once the recognizer commits it; `transcript` is its best hypothesis.
 */
export type WebSpeechResultView = {
	isFinal: boolean;
	transcript: string;
};

/**
 * Minimal, DOM-free view of a `SpeechRecognitionEvent`. `resultIndex` is the
 * lowest index the engine reports as changed since the last event — on Android
 * Chrome it is unreliable and re-delivers already-final results, so the tracker
 * below deliberately does NOT depend on it for correctness.
 */
export type WebSpeechEventView = {
	resultIndex: number;
	results: WebSpeechResultView[];
};

/**
 * Mutable, opaque tracker state recording which final result indices have
 * already been emitted in a recording session. Carried across `onresult`
 * events and reset on stop/restart.
 */
export type WebSpeechFinalTrackerState = {
	emitted: Set<number>;
};

export function createFinalTrackerState(): WebSpeechFinalTrackerState {
	return { emitted: new Set<number>() };
}

/**
 * Pure deduplication for the Web Speech `onresult` stream.
 *
 * The Web Speech API appends results to a stable, index-addressed list and
 * re-delivers already-final results across events (notoriously on Android
 * Chrome, where `resultIndex` advances incorrectly). Each final result must be
 * appended to the Active Capture exactly once across a session, otherwise the
 * transcript multiplies ("yes yes yes and yes and then …").
 *
 * This function walks every result in the event, emits the transcript of any
 * final result whose index has not been emitted yet, and records those indices.
 * It has no DOM or Web Speech dependency, so Vitest can cover it directly.
 *
 * @returns the new transcripts to append (in order) and the next tracker state.
 */
export function consumeFinalResults(
	event: WebSpeechEventView,
	state: WebSpeechFinalTrackerState
): { chunks: string[]; state: WebSpeechFinalTrackerState } {
	const emitted = new Set(state.emitted);
	const chunks: string[] = [];
	for (let i = 0; i < event.results.length; i++) {
		if (emitted.has(i)) continue;
		const result = event.results[i];
		if (!result || !result.isFinal) continue;
		emitted.add(i);
		const transcript = result.transcript.trim();
		if (transcript) chunks.push(transcript);
	}
	return { chunks, state: { emitted } };
}

export class WebSpeechSttSource implements SttSource {
	readonly label = 'web-speech' as const;

	private recognition: SpeechRecognition | null = null;
	private onChunk: ((chunk: string) => void) | null = null;
	private tracker: WebSpeechFinalTrackerState = createFinalTrackerState();

	constructor(private opts: WebSpeechSttSourceOptions = {}) {}

	async start(onChunk: (chunk: string) => void): Promise<void> {
		const ctor = window.SpeechRecognition ?? window.webkitSpeechRecognition;
		if (!ctor) {
			throw new Error('Web Speech API unavailable in this browser');
		}
		this.onChunk = onChunk;
		this.tracker = createFinalTrackerState();
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
			const view: WebSpeechEventView = {
				resultIndex: event.resultIndex,
				results: Array.from(event.results, (r) => ({
					isFinal: r.isFinal,
					transcript: r[0]?.transcript ?? ''
				}))
			};
			const outcome = consumeFinalResults(view, this.tracker);
			this.tracker = outcome.state;
			for (const chunk of outcome.chunks) this.onChunk(chunk + ' ');
		};

		this.recognition = recognition;
		recognition.start();
		await started;
	}

	async stop(): Promise<void> {
		const recognition = this.recognition;
		this.recognition = null;
		this.onChunk = null;
		this.tracker = createFinalTrackerState();
		if (recognition) {
			try {
				recognition.stop();
			} catch {
				/* already stopped */
			}
		}
	}
}
