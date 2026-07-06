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

export class DeepgramSttSource implements SttSource {
	readonly label = 'deepgram' as const;

	private socket: DeepgramLiveSocket | null = null;
	private mediaStream: MediaStream | null = null;
	private audioCtx: AudioContext | null = null;
	private sourceNode: MediaStreamAudioSourceNode | null = null;
	private processor: ScriptProcessorNode | null = null;
	private onChunk: ((chunk: string) => void) | null = null;

	constructor(private opts: DeepgramSttSourceOptions) {}

	async start(onChunk: (chunk: string) => void): Promise<void> {
		this.onChunk = onChunk;
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
				const transcript = msg.channel?.alternatives?.[0]?.transcript ?? '';
				if (transcript) this.onChunk(transcript + ' ');
			}
		});
		this.socket = socket;
		socket.connect();
		await opened;
		await this.beginMic();
	}

	private async beginMic(): Promise<void> {
		const stream = await navigator.mediaDevices.getUserMedia({
			audio: {
				channelCount: 1,
				sampleRate: 16000,
				echoCancellation: true,
				noiseSuppression: true
			}
		});
		this.mediaStream = stream;
		const audioCtx = new AudioContext({ sampleRate: 16000 });
		this.audioCtx = audioCtx;
		const sourceNode = audioCtx.createMediaStreamSource(stream);
		this.sourceNode = sourceNode;
		const processor = audioCtx.createScriptProcessor(4096, 1, 1);
		this.processor = processor;
		processor.onaudioprocess = (e: AudioProcessingEvent) => {
			if (!this.socket) return;
			const float32 = e.inputBuffer.getChannelData(0);
			const int16 = new Int16Array(float32.length);
			for (let i = 0; i < float32.length; i++) {
				const s = Math.max(-1, Math.min(1, float32[i] as number));
				int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
			}
			this.socket.sendMedia(int16);
		};
		sourceNode.connect(processor);
		processor.connect(audioCtx.destination);
	}

	async stop(): Promise<void> {
		if (this.processor) {
			this.processor.disconnect();
			this.processor.onaudioprocess = null;
			this.processor = null;
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
