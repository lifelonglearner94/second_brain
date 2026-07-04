export type TopologySnapshot = {
	fetchedAt: string;
	nodes: { id: string; label: string }[];
	edges: { source: string; target: string; type: string }[];
	partitions: { conceptId: string; cluster: number }[];
};

export type PendingCapture = {
	id: string;
	text: string;
	createdAt: string;
};

const DB_NAME = 'second-brain';
const DB_VERSION = 1;
const SNAPSHOT_STORE = 'topology-snapshot';
const PENDING_STORE = 'pending-captures';
const SNAPSHOT_KEY = 'current';

export interface IdbStore {
	saveTopologySnapshot(snapshot: TopologySnapshot): Promise<void>;
	loadTopologySnapshot(): Promise<TopologySnapshot | undefined>;
	clearTopologySnapshot(): Promise<void>;
	enqueuePendingCapture(capture: PendingCapture): Promise<void>;
	listPendingCaptures(): Promise<PendingCapture[]>;
	removePendingCapture(id: string): Promise<void>;
}

function open(idb: IDBFactory): Promise<IDBDatabase> {
	return new Promise((resolve, reject) => {
		const req = idb.open(DB_NAME, DB_VERSION);
		req.onupgradeneeded = () => {
			const db = req.result;
			if (!db.objectStoreNames.contains(SNAPSHOT_STORE)) {
				db.createObjectStore(SNAPSHOT_STORE);
			}
			if (!db.objectStoreNames.contains(PENDING_STORE)) {
				db.createObjectStore(PENDING_STORE, { keyPath: 'id' });
			}
		};
		req.onsuccess = () => resolve(req.result);
		req.onerror = () => reject(req.error);
	});
}

function tx<T>(db: IDBDatabase, store: string, mode: IDBTransactionMode, run: (s: IDBObjectStore) => IDBRequest<T>): Promise<T> {
	return new Promise((resolve, reject) => {
		const transaction = db.transaction(store, mode);
		const request = run(transaction.objectStore(store));
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => reject(request.error);
		transaction.onerror = () => reject(transaction.error);
	});
}

export function createIdb(idb: IDBFactory = globalThis.indexedDB): IdbStore {
	async function db(): Promise<IDBDatabase> {
		return open(idb);
	}

	return {
		async saveTopologySnapshot(snapshot) {
			const database = await db();
			await tx(database, SNAPSHOT_STORE, 'readwrite', (s) => s.put(snapshot, SNAPSHOT_KEY));
			database.close();
		},
		async loadTopologySnapshot() {
			const database = await db();
			const result = await tx<TopologySnapshot | undefined>(database, SNAPSHOT_STORE, 'readonly', (s) => s.get(SNAPSHOT_KEY));
			database.close();
			return result;
		},
		async clearTopologySnapshot() {
			const database = await db();
			await tx(database, SNAPSHOT_STORE, 'readwrite', (s) => s.delete(SNAPSHOT_KEY));
			database.close();
		},
		async enqueuePendingCapture(capture) {
			const database = await db();
			await tx(database, PENDING_STORE, 'readwrite', (s) => s.add(capture));
			database.close();
		},
		async listPendingCaptures() {
			const database = await db();
			const all = await tx<PendingCapture[]>(database, PENDING_STORE, 'readonly', (s) => s.getAll());
			database.close();
			return all.sort((a, b) => a.createdAt.localeCompare(b.createdAt));
		},
		async removePendingCapture(id) {
			const database = await db();
			await tx(database, PENDING_STORE, 'readwrite', (s) => s.delete(id));
			database.close();
		}
	};
}
