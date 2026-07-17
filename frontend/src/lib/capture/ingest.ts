import type {
	BraindumpDto,
	GraphConcept,
	GraphEdge
} from '$lib/api/client';

export type IngestResponse = {
	braindump: { id: string; created_at: string };
	concepts: GraphConcept[];
	edges: GraphEdge[];
	cursor: number;
};

// Issue #102: the submit hot path is fire-and-forget. The backend persists the
// verbatim in milliseconds and runs clean → extract → accrete in a background
// IngestRunner; the frontend must NOT block on that pipeline. Graph visibility
// for late commits is handled by the /app focus-triggered syncDelta (issue #98)
// and hard-reload (loadFromNetworkOrCache), not by polling here.
//
// `getIngestStatus` stays on ApiClient for diagnostics / optional hints, but
// the submit hot path no longer calls it.
export type IngestClient = {
	submitBraindump(verbatim: string): Promise<BraindumpDto>;
};

export interface IngestApi {
	ingest(verbatim: string): Promise<IngestResponse>;
}

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
	getCursor: () => number
): IngestApi {
	return {
		async ingest(verbatim: string): Promise<IngestResponse> {
			// Fire-and-forget: persist the verbatim and return immediately with
			// an empty delta + the cursor unchanged. The background pipeline's
			// commits are surfaced later by the /app focus-sync (issue #98) or a
			// hard-reload. GraphStore.mergeIngest only advances the cursor when
			// the response carries changes, so an empty response is a no-op on
			// the graph (the #97 cursor-advance invariant).
			const braindump = await client.submitBraindump(verbatim);
			return emptyResponse(braindump, getCursor());
		}
	};
}
