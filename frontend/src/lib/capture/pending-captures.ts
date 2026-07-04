import { createIdb } from '$lib/state/idb';
import { PendingCapturesStore } from '$lib/state/pending-captures.svelte';

export const pendingCaptures = new PendingCapturesStore(createIdb());
