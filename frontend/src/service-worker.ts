import { shouldServeFromCache } from '$lib/service-worker/should-cache';
import { build, files, version } from '$service-worker';

const CACHE = `sb-shell-${version}`;
const appShell: Set<string> = new Set([...build, ...files]);
const appOrigin = self.location.origin;

self.addEventListener('install', (event) => {
	event.waitUntil(
		(async () => {
			const cache = await caches.open(CACHE);
			await cache.addAll([...appShell]);
			await self.skipWaiting();
		})()
	);
});

self.addEventListener('activate', (event) => {
	event.waitUntil(
		(async () => {
			const keys = await caches.keys();
			await Promise.all(
				keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))
			);
			await self.clients.claim();
		})()
	);
});

self.addEventListener('fetch', (event) => {
	if (!shouldServeFromCache(event.request, appShell, appOrigin)) {
		return;
	}
	event.respondWith(
		caches.match(event.request).then((cached) => cached ?? fetch(event.request))
	);
});
