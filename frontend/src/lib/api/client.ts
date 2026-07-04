export type Health = {
	ok: boolean;
	db: boolean;
	sqlite_vec: boolean;
};

export type ApiFetch = typeof fetch;

export interface ApiClientOptions {
	baseUrl?: string;
	fetch?: ApiFetch;
}

const DEFAULT_BASE_URL = '/api';

export interface ApiClient {
	getHealth(): Promise<Health>;
}

export function createApiClient(opts: ApiClientOptions = {}): ApiClient {
	const baseUrl = opts.baseUrl ?? DEFAULT_BASE_URL;
	const doFetch = opts.fetch ?? globalThis.fetch;
	return {
		async getHealth(): Promise<Health> {
			const res = await doFetch(`${baseUrl}/health`, {
				credentials: 'include',
				headers: { accept: 'application/json' }
			});
			if (!res.ok) {
				throw new Error(`GET /health failed: ${res.status}`);
			}
			return (await res.json()) as Health;
		}
	};
}
