import { apiClient } from '$lib/api';
import { AdminLogStore } from '$lib/state/admin-logs.svelte';

export const adminLogs = new AdminLogStore(apiClient);
