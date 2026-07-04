import { createApiClient, type ApiClient, type Health } from './client';

export const apiClient: ApiClient = createApiClient({
	baseUrl: import.meta.env.VITE_BACKEND_BASE_URL ?? '/api'
});

export type { Health };
