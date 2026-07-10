import { describe, it, expect, vi } from 'vitest';
import { ActiveCaptureStore } from '../../src/lib/capture/active-capture.svelte';
import type { SttSource, SttSourceLabel } from '../../src/lib/capture/stt';
import type { IngestApi, IngestResponse } from '../../src/lib/capture/ingest';
import { applyDelta } from '../../src/lib/graph/delta';
import { PendingCapturesStore } from '../../src/lib/state/pending-captures.svelte';
import type { IdbStore, PendingCapture } from '../../src/lib/state/idb';
import { buildGraphData } from '../../src/lib/graph/build';
import type { GlobalTopologySnapshot } from '../../src/lib/api/client';

function fakeDeepgramSource(emits: string[]): SttSource {
	return {
		label: 'deepgram' as SttSourceLabel,
		async start(cb: (chunk: string) => void) {
			for (const chunk of emits) cb(chunk);
		},
		async stop() {}
	};
}

function pendingStore() {
	const enqueued: PendingCapture[] = [];
	const idb: IdbStore = {
		saveTopologySnapshot: vi.fn(),
		loadTopologySnapshot: vi.fn(),
		clearTopologySnapshot: vi.fn(),
		enqueuePendingCapture: vi.fn(async (c: PendingCapture) => {
			enqueued.push(c);
		}),
		listPendingCaptures: vi.fn(async () => [...enqueued]),
		removePendingCapture: vi.fn()
	} as unknown as IdbStore;
	return new PendingCapturesStore(idb);
}

const SNAPSHOT: GlobalTopologySnapshot = {
	concepts: [{ id: 'c1', label: 'sleep', created_at: '100' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
};

describe('Active Capture vertical slice - STT → buffer → explicit submit → optimistic merge (ADR-0002/0007)', () => {
	it('streams Deepgram chunks into the buffer, submits on explicit submit, and merges the ingested concepts/edges into the Spatial View-Graph', async () => {
		const store = new ActiveCaptureStore();
		const ingested: IngestResponse = {
			braindump: { id: 'b1', created_at: '200' },
			concepts: [{ id: 'c2', label: 'caffeine', created_at: '200' }],
			edges: [
				{
					id: 'e1',
					source_concept_id: 'c2',
					target_concept_id: 'c1',
					original_type: 'disrupts',
					current_type: 'disrupts',
					created_at: '200'
				}
			],
			cursor: 200
		};

		await store.startStt(fakeDeepgramSource(['caffeine ', 'disrupts sleep']));
		expect(store.text).toBe('caffeine disrupts sleep');

		const outcome = await store.submit(
			makeIngest(ingested),
			true,
			pendingStore()
		);
		expect(outcome.kind).toBe('submitted');
		if (outcome.kind !== 'submitted')
			throw new Error('expected submitted outcome');
		const res = outcome.res;
		expect(res.concepts[0]?.label).toBe('caffeine');

		expect(store.text).toBe('');
		expect(store.status).toBe('submitted');

		const merged = applyDelta(SNAPSHOT, {
			cursor: res.cursor,
			added_concepts: res.concepts,
			added_edges: res.edges,
			deleted_concept_ids: [],
			deleted_edge_ids: [],
			retagged_edges: []
		});
		const data = buildGraphData(merged);
		expect(data.nodes.map((n) => n.id).sort()).toEqual(['c1', 'c2']);
		expect(data.links.map((l) => `${l.source}-${l.target}`)).toContain('c2-c1');
	});

	it('falls back from an unreachable Deepgram source to Web Speech and still feeds the buffer through to submit', async () => {
		const store = new ActiveCaptureStore();
		const deepgram: SttSource = {
			label: 'deepgram',
			async start() {
				throw new Error('deepgram unreachable');
			},
			async stop() {}
		};
		const webSpeech: SttSource = {
			label: 'web-speech',
			async start(cb) {
				cb('offline ');
				cb('capture');
			},
			async stop() {}
		};

		const label = await store.startCaptureWithFallback(deepgram, webSpeech);
		expect(label).toBe('web-speech');
		expect(store.text).toBe('offline capture');

		const outcome = await store.submit(
			makeIngest({
				braindump: { id: 'b2', created_at: '300' },
				concepts: [{ id: 'c3', label: 'offline-thought', created_at: '300' }],
				edges: [],
				cursor: 300
			}),
			true,
			pendingStore()
		);
		expect(outcome.kind).toBe('queued');
		expect(store.text).toBe('');
		expect(store.status).toBe('queued');
	});

	it('does not ingest until the user explicitly submits - chunk arrival alone never POSTs', async () => {
		const store = new ActiveCaptureStore();
		const calls: string[] = [];
		const ingest: IngestApi = {
			async ingest(verbatim) {
				calls.push(verbatim);
				return {
					braindump: { id: 'b', created_at: '1' },
					concepts: [],
					edges: [],
					cursor: 1
				};
			}
		};
		await store.startStt(fakeDeepgramSource(['a chunk']));
		expect(calls).toHaveLength(0);
		await store.submit(ingest, true, pendingStore());
		expect(calls).toEqual(['a chunk']);
	});
});

function makeIngest(res: IngestResponse): IngestApi {
	return {
		async ingest() {
			return res;
		}
	};
}
