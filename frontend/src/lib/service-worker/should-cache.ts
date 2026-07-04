export function shouldServeFromCache(
	request: Request,
	cachedPaths: ReadonlySet<string>,
	appOrigin: string
): boolean {
	if (request.method !== 'GET') return false;
	const url = new URL(request.url);
	if (url.origin !== appOrigin) return false;
	if (url.pathname.startsWith('/api/')) return false;
	return cachedPaths.has(url.pathname);
}
