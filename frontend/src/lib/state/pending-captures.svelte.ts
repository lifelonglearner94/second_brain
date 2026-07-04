import { createIdb } from './idb';
import type { IdbStore, PendingCapture } from './idb';

export class PendingCapturesStore {
	items = $state<PendingCapture[]>([]);
	count = $derived(this.items.length);

	constructor(private idb: IdbStore) {}

	async load(): Promise<void> {
		this.items = await this.idb.listPendingCaptures();
	}

	async enqueue(capture: PendingCapture): Promise<void> {
		await this.idb.enqueuePendingCapture(capture);
		this.items = [...this.items, capture];
	}

	async remove(id: string): Promise<void> {
		await this.idb.removePendingCapture(id);
		this.items = this.items.filter((c) => c.id !== id);
	}
}

export const pendingCaptures = new PendingCapturesStore(createIdb());
