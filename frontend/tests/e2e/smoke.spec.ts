import { expect, test } from '@playwright/test';

const HEALTH_BODY = { ok: true, db: true, sqlite_vec: true };

test('smoke: PWA shell loads and reaches backend #1 GET /health through the API client', async ({
	page
}) => {
	await page.route('**/api/health', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(HEALTH_BODY)
		})
	);

	await page.goto('/');

	await expect(page.getByTestId('health-ok')).toContainText('healthy', { timeout: 10_000 });
});

test('smoke: the PWA manifest is served so the app is installable', async ({ request }) => {
	const res = await request.get('/manifest.webmanifest');
	expect(res.status()).toBe(200);
	const manifest = await res.json();
	expect(manifest.name).toBe('Second Brain');
	expect(manifest.icons.some((i: { sizes: string }) => i.sizes === '192x192')).toBeTruthy();
	expect(manifest.icons.some((i: { sizes: string }) => i.sizes === '512x512')).toBeTruthy();
});

test('smoke: the dumb Service Worker registers and activates (app-shell cache, no business logic)', async ({
	page
}) => {
	await page.route('**/api/health', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(HEALTH_BODY)
		})
	);

	await page.goto('/');
	await expect.poll(
		async () => {
			return page.evaluate(async () => {
				const reg = await navigator.serviceWorker.getRegistration();
				return reg?.active?.state ?? null;
			});
		},
		{ timeout: 10_000 }
	).toBe('activated');
});
