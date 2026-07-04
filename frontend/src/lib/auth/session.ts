import { apiClient } from '$lib/api';
import { SessionStore } from '$lib/state/session.svelte';

export const session = new SessionStore(apiClient);
