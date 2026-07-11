import type { SttSource } from './stt';

type DeepgramMessage = {
	is_final?: boolean;
	channel?: { alternatives?: { transcript?: string }[] };
};

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

/**
 * Build the WebSocket URL for the backend Deepgram proxy, respecting
 * VITE_BACKEND_BASE_URL when set (like the rest of the API client — see
 * `src/lib/api/index.ts` which uses `VITE_BACKEND_BASE_URL ?? '/api'`).
 *
 * When the base is `/api` (default, Caddy-proxied), the URL is
 * `wss://<host>/api/stt/deepgram` — Caddy strips the `/api/` prefix so the
 * backend sees `/stt/deepgram`.
 *
 * When the base is an absolute URL (e.g. `http://127.0.0.1:8080` for local
 * dev against a bare backend), the URL is `ws://127.0.0.1:8080/stt/deepgram`
 * — no `/api/` prefix, because the backend routes are at the root.
 */
export function buildProxyUrl(opts: {
	backendBase?: string;
	location: { protocol: string; host: string };
}): string {
	const base = opts.backendBase ?? '/api';
	const httpUrl = new URL(base, `${opts.location.protocol}//${opts.location.host}`);
	const protocol = httpUrl.protocol === 'https:' ? 'wss:' : 'ws:';
	const path = httpUrl.pathname.replace(/\/$/, '');
	return `${protocol}//${httpUrl.host}${path}/stt/deepgram`;
}

export class DeepgramSttSource implements SttSource {
	readonly label = 'deepgram' as const;

	private socket: WebSocket | null = null;
	private mediaStream: MediaStream | null = null;
	private audioCtx: AudioContext | null = null;
	private sourceNode: MediaStreamAudioSourceNode | null = null;
	private processor: ScriptProcessorNode | null = null;
	private muteGain: GainNode | null = null;
	private onChunk: ((chunk: string) => void) | null = null;

	async start(onChunk: (chunk: string) => void): Promise<void> {
		this.onChunk = onChunk;

		// iOS Safari: an AudioContext starts suspended and only resumes when
		// resume() is called within a user gesture. Create it and kick off
		// resume() synchronously here - before any await - so the call happens
		// inside the Record tap's transient activation window (the awaits below
		// for the WebSocket open would otherwise leave the gesture and leave the
		// context suspended, producing silence).
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
			const socket = new WebSocket(
				buildProxyUrl({
					backendBase: import.meta.env.VITE_BACKEND_BASE_URL,
					location: window.location
				})
			);
			this.socket = socket;

			const opened = new Promise<void>((resolve, reject) => {
				socket.onopen = () => resolve();
				socket.onerror = () =>
					reject(new Error('Failed to connect to Deepgram proxy'));
			});

			socket.onmessage = (event: MessageEvent) => {
				if (!this.onChunk || typeof event.data !== 'string') return;
				try {
					const msg: DeepgramMessage = JSON.parse(event.data);
					if (msg.is_final) {
						const transcript =
							msg.channel?.alternatives?.[0]?.transcript ?? '';
						if (transcript) this.onChunk(transcript + ' ');
					}
				} catch {
					// Ignore malformed JSON
				}
			};

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
			if (!this.socket || this.socket.readyState !== WebSocket.OPEN) return;
			const float32 = e.inputBuffer.getChannelData(0);
			const resampled = resampleTo16kMono(float32, inputRate);
			const int16 = new Int16Array(resampled.length);
			for (let i = 0; i < resampled.length; i++) {
				const s = Math.max(-1, Math.min(1, resampled[i] as number));
				int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
			}
			this.socket.send(int16.buffer);
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