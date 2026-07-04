import { apiClient } from '$lib/api';
import { HousekeepingStore } from '$lib/state/housekeeping.svelte';

export const housekeeping = new HousekeepingStore(apiClient);

