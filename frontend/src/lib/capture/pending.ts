import type { SttSourceLabel } from './stt';
import type { IngestApi, IngestResponse } from './ingest';
import type { ActiveCaptureStore } from './active-capture.svelte';
import type { PendingCapture } from '$lib/state/idb';
import type { PendingCapturesStore } from '$lib/state/pending-captures.svelte';

export function shouldQueuePending(
	online: boolean,
	sttSourceLabel: SttSourceLabel | null
): boolean {
	return !online || sttSourceLabel === 'web-speech';
}

export type CaptureSubmission =
	| { kind: 'submitted'; res: IngestResponse }
	| { kind: 'queued' };

export async function submitActiveCapture(
	active: ActiveCaptureStore,
	online: boolean,
	pending: PendingCapturesStore,
	ingest: IngestApi
): Promise<CaptureSubmission> {
	const verbatim = active.text;
	if (verbatim.trim().length === 0) {
		throw new Error('Active Capture buffer is empty — nothing to submit');
	}
	if (shouldQueuePending(online, active.sttSourceLabel)) {
		const capture: PendingCapture = {
			id: crypto.randomUUID(),
			text: verbatim,
			createdAt: new Date().toISOString()
		};
		await pending.enqueue(capture);
		await active.stopStt();
		active.clear();
		active.status = 'queued';
		return { kind: 'queued' };
	}
	const res = await active.submit(ingest);
	return { kind: 'submitted', res };
}
