import type {
	BraindumpDto,
	GraphDelta,
	GraphConcept,
	GraphEdge
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
};

export interface IngestApi {
	ingest(verbatim: string): Promise<IngestResponse>;
}

export function createIngestApi(
	client: IngestClient,
	getCursor: () => number
): IngestApi {
	return {
		async ingest(verbatim: string): Promise<IngestResponse> {
			const braindump = await client.submitBraindump(verbatim);
			const delta = await client.getGraphDelta(getCursor());
			return {
				braindump: { id: braindump.id, created_at: braindump.created_at },
				concepts: delta.added_concepts,
				edges: delta.added_edges,
				cursor: delta.cursor
			};
		}
	};
}
