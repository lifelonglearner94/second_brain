import { apiClient } from '$lib/api';
import { EndorsementStore } from '$lib/state/endorsement-queue.svelte';
import { spatialGraph } from '$lib/state/spatial-graph.svelte';

export const endorsementQueue = new EndorsementStore(apiClient, spatialGraph);
