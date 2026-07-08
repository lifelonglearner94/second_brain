import { describe, it, expect } from 'vitest';
import {
	consumeFinalResults,
	createFinalTrackerState
} from './web-speech';
import type { WebSpeechEventView } from './web-speech';

function event(
	resultIndex: number,
	results: { isFinal: boolean; transcript: string }[]
): WebSpeechEventView {
	return { resultIndex, results };
}

/**
 * Drives a sequence of `onresult` events through the dedup tracker and joins
 * the emitted transcripts the way `WebSpeechSttSource` appends them to the
 * Active Capture (space-separated chunks).
 */
function transcribe(events: WebSpeechEventView[]): string {
	let state = createFinalTrackerState();
	const out: string[] = [];
	for (const ev of events) {
		const res = consumeFinalResults(ev, state);
		state = res.state;
		for (const chunk of res.chunks) out.push(chunk);
	}
	return out.join(' ');
}

describe('consumeFinalResults — Web Speech final-result dedup (issue #83)', () => {
	it('emits each final result exactly once in a normal stream', () => {
		const text = transcribe([
			event(0, [{ isFinal: true, transcript: 'ja' }]),
			event(1, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und dann' }
			]),
			event(2, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und dann' },
				{ isFinal: true, transcript: 'habe ich' }
			])
		]);
		expect(text).toBe('ja und dann habe ich');
	});

	it('does not multiply words when Android Chrome re-delivers finals with a stuck resultIndex', () => {
		// Android Chrome re-delivers the whole final list each event while
		// resultIndex stays at 0. Without dedup this yields
		// "ja ja ja und ja und dann ja und dann habe ich".
		const text = transcribe([
			event(0, [{ isFinal: true, transcript: 'ja' }]),
			event(0, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und' }
			]),
			event(0, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und' },
				{ isFinal: true, transcript: 'dann' }
			]),
			event(0, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und' },
				{ isFinal: true, transcript: 'dann' },
				{ isFinal: true, transcript: 'habe ich' }
			])
		]);
		expect(text).toBe('ja und dann habe ich');
	});

	it('reproduces the reported sentence once instead of the multiplied form', () => {
		// Issue report: "yes and then I have" came back as
		// "yes yes yes yes and yes and then yes and then have …"
		const text = transcribe([
			event(0, [{ isFinal: true, transcript: 'yes' }]),
			event(0, [
				{ isFinal: true, transcript: 'yes' },
				{ isFinal: true, transcript: 'and' }
			]),
			event(0, [
				{ isFinal: true, transcript: 'yes' },
				{ isFinal: true, transcript: 'and' },
				{ isFinal: true, transcript: 'then' }
			]),
			event(0, [
				{ isFinal: true, transcript: 'yes' },
				{ isFinal: true, transcript: 'and' },
				{ isFinal: true, transcript: 'then' },
				{ isFinal: true, transcript: 'I have' }
			])
		]);
		expect(text).toBe('yes and then I have');
	});

	it('skips interim results and only emits once they become final', () => {
		let state = createFinalTrackerState();
		const out: string[] = [];

		// index 0 interim, index 1 final
		let r = consumeFinalResults(
			event(0, [
				{ isFinal: false, transcript: 'ja' },
				{ isFinal: true, transcript: 'und' }
			]),
			state
		);
		state = r.state;
		for (const c of r.chunks) out.push(c);
		expect(out).toEqual(['und']);

		// index 0 now becomes final — must be emitted exactly once
		r = consumeFinalResults(
			event(0, [
				{ isFinal: true, transcript: 'ja' },
				{ isFinal: true, transcript: 'und' }
			]),
			state
		);
		state = r.state;
		for (const c of r.chunks) out.push(c);
		expect(out).toEqual(['und', 'ja']);
		expect(state.emitted.has(0)).toBe(true);
		expect(state.emitted.has(1)).toBe(true);
	});

	it('does not re-emit an interim-turned-final on later re-delivery', () => {
		const text = transcribe([
			event(0, [{ isFinal: false, transcript: 'hallo' }]),
			event(0, [{ isFinal: true, transcript: 'hallo' }]),
			event(0, [{ isFinal: true, transcript: 'hallo' }]),
			event(0, [
				{ isFinal: true, transcript: 'hallo' },
				{ isFinal: true, transcript: 'welt' }
			])
		]);
		expect(text).toBe('hallo welt');
	});

	it('skips empty/whitespace transcripts without re-emitting them later', () => {
		const text = transcribe([
			event(0, [{ isFinal: true, transcript: '   ' }]),
			event(0, [
				{ isFinal: true, transcript: '   ' },
				{ isFinal: true, transcript: 'wort' }
			])
		]);
		expect(text).toBe('wort');
	});

	it('reset (fresh state) clears carry-over so restart does not duplicate', () => {
		// First session
		let state = createFinalTrackerState();
		let r = consumeFinalResults(
			event(0, [{ isFinal: true, transcript: 'eins' }]),
			state
		);
		state = r.state;
		expect(r.chunks).toEqual(['eins']);

		// Stop + restart: brand-new state. Re-delivering index 0 must emit again
		// because it is a new session (new recognition, fresh result list).
		state = createFinalTrackerState();
		r = consumeFinalResults(
			event(0, [{ isFinal: true, transcript: 'zwei' }]),
			state
		);
		expect(r.chunks).toEqual(['zwei']);
	});

	it('returns a new state and leaves the previous state untouched (purity)', () => {
		const state = createFinalTrackerState();
		const before = state.emitted.size;
		consumeFinalResults(
			event(0, [{ isFinal: true, transcript: 'a' }]),
			state
		);
		expect(state.emitted.size).toBe(before);
	});

	it('is unaffected by an unreliable resultIndex (iterates all results)', () => {
		// resultIndex claims nothing changed, but a new final exists at index 1.
		const text = transcribe([
			event(0, [{ isFinal: true, transcript: 'a' }]),
			event(0, [
				{ isFinal: true, transcript: 'a' },
				{ isFinal: true, transcript: 'b' }
			])
		]);
		expect(text).toBe('a b');
	});
});
