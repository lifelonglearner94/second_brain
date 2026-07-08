import { apiClient } from '$lib/api';
import type { SystemResponse } from '$lib/api/client';

export type AdminSystemStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type AdminSystemApi = {
	getSystem(): Promise<SystemResponse>;
};

/**
 * Admin tab state for the server-load view (backend #81). Pulls current host
 * metrics — CPU, RAM, disk — from `GET /admin/system` so the operator reads VPS
 * pressure from the phone without SSH. The store holds the latest snapshot and
 * re-fetches on demand; the metrics are instantaneous readings (no live
 * streaming), mirroring the pull-based admin-logs store. The backend owns no
 * formatting: raw bytes and percentages arrive here and the page formats
 * human-friendly sizes.
 */
export class AdminSystemStore {
	status = $state<AdminSystemStatus>('idle');
	metrics = $state<SystemResponse | null>(null);
	error = $state<string | null>(null);

	constructor(private api: AdminSystemApi) {}

	async refresh(): Promise<void> {
		this.status = 'loading';
		this.error = null;
		try {
			this.metrics = await this.api.getSystem();
			this.status = 'loaded';
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
		}
	}
}

export const adminSystem = new AdminSystemStore(apiClient);
