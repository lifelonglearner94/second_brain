import type { LogEntry, LogsResponse } from '$lib/api/client';

export type AdminLogStatus = 'idle' | 'loading' | 'loaded' | 'error';

export type AdminLogApi = {
	getAdminLogs(limit?: number): Promise<LogsResponse>;
};

export class AdminLogStore {
	status = $state<AdminLogStatus>('idle');
	logs = $state<LogEntry[]>([]);
	count = $state(0);
	capacity = $state(0);
	error = $state<string | null>(null);

	levelFilter = $state<string>('all');
	query = $state<string>('');

	constructor(private api: AdminLogApi) {}

	async refresh(limit?: number): Promise<void> {
		this.status = 'loading';
		this.error = null;
		try {
			const res = await this.api.getAdminLogs(limit);
			this.logs = res.logs;
			this.count = res.count;
			this.capacity = res.capacity;
			this.status = 'loaded';
		} catch (e) {
			this.error = e instanceof Error ? e.message : String(e);
			this.status = 'error';
		}
	}

	levels = $derived.by<string[]>(() => {
		const seen = new Set<string>();
		for (const l of this.logs) seen.add(l.level);
		return [...seen];
	});

	filtered = $derived.by<LogEntry[]>(() => {
		const q = this.query.trim().toLowerCase();
		return this.logs.filter((l) => {
			if (this.levelFilter !== 'all' && l.level !== this.levelFilter) return false;
			if (q) {
				const hay = `${l.message} ${l.target} ${JSON.stringify(l.fields)}`.toLowerCase();
				if (!hay.includes(q)) return false;
			}
			return true;
		});
	});
}
