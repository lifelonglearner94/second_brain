import type { SttSourceLabel } from './stt';

export function shouldQueuePending(
	online: boolean,
	sttSourceLabel: SttSourceLabel | null
): boolean {
	return !online || sttSourceLabel === 'web-speech';
}
