import type { SttSource, SttSourceLabel } from './stt';
import type { IngestApi, IngestResponse } from './ingest';
import type { PendingCapturesStore } from '$lib/state/pending-captures.svelte';
import type { PendingCapture } from '$lib/state/idb';

export type ActiveCaptureStatus =
	'idle' | 'listening' | 'submitting' | 'submitted' | 'queued' | 'error';

export type CaptureSubmission =
	{ kind: 'submitted'; res: IngestResponse } | { kind: 'queued' };

export class ActiveCaptureStore {
	text = $state<string>('');
	status = $state<ActiveCaptureStatus>('idle');
	error = $state<string | null>(null);
	sttSourceLabel = $state<SttSourceLabel | null>(null);

	private currentSource: SttSource | null = null;

	appendSttChunk(chunk: string): void {
		if (chunk) this.text += chunk;
	}

	setText(text: string): void {
		this.text = text;
	}

	clear(): void {
		this.text = '';
		this.error = null;
	}

	async startStt(source: SttSource): Promise<void> {
		this.currentSource = source;
		this.sttSourceLabel = source.label;
		this.status = 'listening';
		try {
			await source.start((chunk: string) => this.appendSttChunk(chunk));
		} catch (e) {
			this.currentSource = null;
			this.sttSourceLabel = null;
			this.status = 'error';
			this.error = e instanceof Error ? e.message : String(e);
			throw e;
		}
	}

	async stopStt(): Promise<void> {
		await this.stopCurrentSource();
		if (this.status === 'listening') this.status = 'idle';
	}

	async startCaptureWithFallback(
		primary: SttSource,
		fallback: SttSource | null
	): Promise<SttSourceLabel> {
		try {
			await this.startStt(primary);
			return primary.label;
		} catch (primaryError) {
			if (fallback) {
				await this.startStt(fallback);
				return fallback.label;
			}
			this.status = 'error';
			this.error =
				primaryError instanceof Error
					? primaryError.message
					: String(primaryError);
			throw primaryError;
		}
	}

	async submit(
		ingest: IngestApi,
		online: boolean,
		pending: PendingCapturesStore
	): Promise<CaptureSubmission> {
		const verbatim = this.text;
		if (verbatim.trim().length === 0) {
			throw new Error('Active Capture buffer is empty - nothing to submit');
		}
		if (this.shouldQueuePending(online)) {
			const capture: PendingCapture = {
				id: crypto.randomUUID(),
				text: verbatim,
				createdAt: new Date().toISOString()
			};
			await pending.enqueue(capture);
			await this.stopStt();
			this.clear();
			this.status = 'queued';
			return { kind: 'queued' };
		}
		this.status = 'submitting';
		try {
			const res = await ingest.ingest(verbatim);
			await this.stopCurrentSource();
			this.text = '';
			this.error = null;
			this.status = 'submitted';
			return { kind: 'submitted', res };
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
			throw e;
		}
	}

	private shouldQueuePending(online: boolean): boolean {
		return !online || this.sttSourceLabel === 'web-speech';
	}

	private async stopCurrentSource(): Promise<void> {
		const source = this.currentSource;
		this.currentSource = null;
		this.sttSourceLabel = null;
		if (source) {
			try {
				await source.stop();
			} catch {
				/* source cleanup is best-effort */
			}
		}
	}
}
