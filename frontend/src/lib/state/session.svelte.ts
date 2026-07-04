import type { Me } from '$lib/api/client';

export type SessionStatus = 'unknown' | 'authenticated' | 'unauthenticated';

export type SessionApi = {
	getMe(): Promise<Me>;
};

export class SessionStore {
	status = $state<SessionStatus>('unknown');
	userId = $state<string | null>(null);

	constructor(private api: SessionApi) {}

	async refresh(): Promise<void> {
		try {
			const me = await this.api.getMe();
			this.status = 'authenticated';
			this.userId = me.user_id;
		} catch {
			this.status = 'unauthenticated';
			this.userId = null;
		}
	}

	setAuthenticated(userId: string): void {
		this.status = 'authenticated';
		this.userId = userId;
	}

	clear(): void {
		this.status = 'unauthenticated';
		this.userId = null;
	}
}
