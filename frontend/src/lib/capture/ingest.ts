import type {
	BraindumpDto,
	GraphDelta,
	GraphConcept,
	GraphEdge,
	IngestStatus
} from '$lib/api/client';

export type IngestResponse = {
	braindump: { id: string; created_at: string };
	concepts: GraphConcept[];
	edges: GraphEdge[];
	cursor: number;
};

export type IngestClient = {
	submitBraindump(verbatim: string): Promise<BraindumpDto>;
	getGraphDelta(since?: number): Promise<GraphDelta>;
	getIngestStatus(id: number): Promise<IngestStatus>;
};

export interface IngestApi {
	ingest(verbatim: string): Promise<IngestResponse>;
}

// Issue #97: backoff schedule for the ingest-status poll. Each delay doubles
// the previous; the cumulative cap (~12s) bounds the poll so a stuck
// background pipeline does not block the UI forever. Kept as a module-level
// constant so tests can substitute short delays via the `delays` parameter.
const DEFAULT_POLL_DELAYS_MS = [400, 800, 1600, 3200, 6400];

function emptyResponse(
	braindump: BraindumpDto,
	cursor: number
): IngestResponse {
	return {
		braindump: { id: braindump.id, created_at: braindump.created_at },
		concepts: [],
		edges: [],
		cursor
	};
}

export function createIngestApi(
	client: IngestClient,
	getCursor: () => number,
	delays: number[] = DEFAULT_POLL_DELAYS_MS
): IngestApi {
	return {
		async ingest(verbatim: string): Promise<IngestResponse> {
			const braindump = await client.submitBraindump(verbatim);
			const id = Number(braindump.id);

			// Issue #97: poll the backend ingest-status until the background
			// clean → extract → accrete pipeline commits (or fails, or the
			// backoff budget runs out). The single racing `getGraphDelta` the
			// old code fired immediately is replaced by a final delta pull on
			// `complete`; on `failed`/timeout the cursor is NOT advanced so
			// the next focus/submit sync still catches the background commit.
			for (const delay of delays) {
				const status = await client.getIngestStatus(id);
				if (status.status === 'complete') {
					const delta = await client.getGraphDelta(getCursor());
					return {
						braindump: {
							id: braindump.id,
							created_at: braindump.created_at
						},
						concepts: delta.added_concepts,
						edges: delta.added_edges,
						cursor: delta.cursor
					};
				}
				if (status.status === 'failed') {
					return emptyResponse(braindump, getCursor());
				}
				await new Promise((r) => setTimeout(r, delay));
			}
			// Backoff budget exhausted: the background pipeline has not
			// committed yet. Stop without advancing the cursor so the next
			// focus/submit sync still catches it.
			return emptyResponse(braindump, getCursor());
		}
	};
}
