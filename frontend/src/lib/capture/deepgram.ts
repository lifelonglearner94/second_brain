import type { SttSource } from './stt';

export type DeepgramSttSourceOptions = {
	apiKey: string;
	model?: string;
	language?: string;
};

type DeepgramMessage = {
	is_final?: boolean;
	channel?: { alternatives?: { transcript?: string }[] };
};

interface DeepgramLiveSocket {
	on(event: 'open', cb: () => void): void;
	on(event: 'message', cb: (msg: DeepgramMessage) => void): void;
	on(event: 'error', cb: (err: unknown) => void): void;
	on(event: 'close', cb: () => void): void;
	connect(): void;
	waitForOpen(): Promise<unknown>;
	sendMedia(data: ArrayBuffer | ArrayBufferView | Blob): void;
	close(): void;
}

/**
 * Resample a mono Float32 PCM buffer to 16 kHz. Pure and DOM-free so Vitest can
 * cover it. iOS Safari may ignore `new AudioContext({ sampleRate: 16000 })` and
 * run the graph at 48 kHz; the ScriptProcessor then hands us 48 kHz frames that
 * Deepgram (told `sample_rate: 16000`) would misinterpret. We resample to the
 * rate we promised Deepgram regardless of the context's actual rate. When the
 * context already runs at 16 kHz (desktop Chrome, Android Chrome) the input is
 * returned unchanged.
 */
export function resampleTo16kMono(
	input: Float32Array,
	inputRate: number
): Float32Array {
	const targetRate = 16000;
	if (input.length === 0 || inputRate === targetRate) return input;
	const ratio = inputRate / targetRate;
	const outLength = Math.max(1, Math.floor(input.length / ratio));
	const out = new Float32Array(outLength);
	for (let i = 0; i < outLength; i++) {
		const srcIndex = i * ratio;
		const lo = Math.floor(srcIndex);
		const hi = Math.min(lo + 1, input.length - 1);
		const frac = srcIndex - lo;
		const a = input[lo] ?? 0;
		const b = input[hi] ?? 0;
		out[i] = a + (b - a) * frac;
	}
	return out;
}

/**
 * Resolve the AudioContext constructor, falling back to the legacy webkit-prefixed
 * form used by older iOS Safari.
 */
function audioContextCtor(): typeof AudioContext | null {
	if (typeof window === 'undefined') return null;
	return (
		window.AudioContext ??
		(window as unknown as { webkitAudioContext?: typeof AudioContext })
			.webkitAudioContext ??
		null
	);
}

export class DeepgramSttSource implements SttSource {
	readonly label = 'deepgram' as const;

	private socket: DeepgramLiveSocket | null = null;
	private mediaStream: MediaStream | null = null;
	private audioCtx: AudioContext | null = null;
	private sourceNode: MediaStreamAudioSourceNode | null = null;
	private processor: ScriptProcessorNode | null = null;
	private muteGain: GainNode | null = null;
	private onChunk: ((chunk: string) => void) | null = null;

	constructor(private opts: DeepgramSttSourceOptions) {}

	async start(onChunk: (chunk: string) => void): Promise<void> {
		this.onChunk = onChunk;

		// iOS Safari: an AudioContext starts suspended and only resumes when
		// resume() is called within a user gesture. Create it and kick off
		// resume() synchronously here — before any await — so the call happens
		// inside the Record tap's transient activation window (the awaits below
		// for the SDK import and the WebSocket open would otherwise leave the
		// gesture and leave the context suspended, producing silence).
		const Ctor = audioContextCtor();
		if (!Ctor) {
			throw new Error('AudioContext unavailable in this browser');
		}
		const audioCtx = new Ctor({ sampleRate: 16000 });
		this.audioCtx = audioCtx;
		// Fire resume() without awaiting; the in-gesture *call* is what iOS
		// requires. We re-resume after the mic is wired as a safety net.
		void audioCtx.resume().catch(() => {
			/* settled below / on first frame */
		});

		try {
			const { DeepgramClient } = await import('@deepgram/sdk');
			const client = new DeepgramClient({ apiKey: this.opts.apiKey });
			const socket = (await client.listen.v1.connect({
				model: this.opts.model ?? 'nova-3',
				language: this.opts.language ?? 'de',
				encoding: 'linear16',
				sample_rate: 16000,
				channels: 1,
				interim_results: 'true',
				smart_format: 'true',
				Authorization: this.opts.apiKey
			})) as unknown as DeepgramLiveSocket;

			const opened = new Promise<void>((resolve, reject) => {
				socket.on('open', () => resolve());
				socket.on('error', (err: unknown) =>
					reject(
						err instanceof Error
							? err
							: new Error(`Deepgram unreachable: ${String(err)}`)
					)
				);
			});
			socket.on('message', (msg: DeepgramMessage) => {
				if (!this.onChunk) return;
				if (msg.is_final) {
					const transcript =
						msg.channel?.alternatives?.[0]?.transcript ?? '';
					if (transcript) this.onChunk(transcript + ' ');
				}
			});
			this.socket = socket;
			socket.connect();
			await opened;
			await this.beginMic();
		} catch (e) {
			// Release the AudioContext (and any mic) we created so a failed
			// start does not leak a suspended context or a live mic track.
			await this.stop().catch(() => {
				/* best-effort cleanup */
			});
			throw e;
		}
	}

	private async beginMic(): Promise<void> {
		const audioCtx = this.audioCtx;
		const socket = this.socket;
		if (!audioCtx || !socket) return;

		const stream = await navigator.mediaDevices.getUserMedia({
			audio: {
				channelCount: 1,
				sampleRate: 16000,
				echoCancellation: true,
				noiseSuppression: true
			}
		});
		this.mediaStream = stream;

		// Re-resume in case the context auto-suspended (some iOS versions do
		// this between the gesture and mic acquisition). Harmless no-op on
		// desktop where it is already running.
		if (audioCtx.state === 'suspended') {
			await audioCtx.resume().catch(() => {});
		}

		const sourceNode = audioCtx.createMediaStreamSource(stream);
		this.sourceNode = sourceNode;
		const processor = audioCtx.createScriptProcessor(4096, 1, 1);
		this.processor = processor;
		// Route through a zero-gain node so onaudioprocess fires (the graph must
		// reach the destination on Safari) without playing the mic back through
		// the speaker, which would cause echo/feedback on iOS and desktop alike.
		const muteGain = audioCtx.createGain();
		muteGain.gain.value = 0;
		this.muteGain = muteGain;

		const inputRate = audioCtx.sampleRate;
		processor.onaudioprocess = (e: AudioProcessingEvent) => {
			if (!this.socket) return;
			const float32 = e.inputBuffer.getChannelData(0);
			const resampled = resampleTo16kMono(float32, inputRate);
			const int16 = new Int16Array(resampled.length);
			for (let i = 0; i < resampled.length; i++) {
				const s = Math.max(-1, Math.min(1, resampled[i] as number));
				int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
			}
			this.socket.sendMedia(int16);
		};
		sourceNode.connect(processor);
		processor.connect(muteGain);
		muteGain.connect(audioCtx.destination);
	}

	async stop(): Promise<void> {
		if (this.processor) {
			this.processor.disconnect();
			this.processor.onaudioprocess = null;
			this.processor = null;
		}
		if (this.muteGain) {
			this.muteGain.disconnect();
			this.muteGain = null;
		}
		if (this.sourceNode) {
			this.sourceNode.disconnect();
			this.sourceNode = null;
		}
		if (this.audioCtx) {
			try {
				await this.audioCtx.close();
			} catch {
				/* already closed */
			}
			this.audioCtx = null;
		}
		if (this.mediaStream) {
			for (const track of this.mediaStream.getTracks()) track.stop();
			this.mediaStream = null;
		}
		if (this.socket) {
			try {
				this.socket.close();
			} catch {
				/* already closed */
			}
			this.socket = null;
		}
		this.onChunk = null;
	}
}
