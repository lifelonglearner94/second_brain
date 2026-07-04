export class OnlineStore {
	online = $state(typeof navigator !== 'undefined' ? navigator.onLine : true);

	init(): () => void {
		const update = (): void => {
			this.online = typeof navigator !== 'undefined' ? navigator.onLine : true;
		};
		globalThis.addEventListener('online', update);
		globalThis.addEventListener('offline', update);
		return () => {
			globalThis.removeEventListener('online', update);
			globalThis.removeEventListener('offline', update);
		};
	}
}
