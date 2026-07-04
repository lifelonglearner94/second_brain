import { expect, test } from '@playwright/test';

const USER_ID = '00000000-0000-0000-0000-000000000001';

const GRAPH_BODY = {
	concepts: [{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
};

function mockMe(page: import('@playwright/test').Page, status: number, body: unknown) {
	return page.route('**/api/me', (route) =>
		route.fulfill({
			status,
			contentType: 'application/json',
			body: JSON.stringify(body)
		})
	);
}

function mockGraph(page: import('@playwright/test').Page, status: number, body: unknown) {
	return page.route('**/api/graph', (route) =>
		route.fulfill({
			status,
			contentType: 'application/json',
			body: JSON.stringify(body)
		})
	);
}

test('unauthenticated: visiting /app redirects to /login when /me rejects with 401', async ({
	page
}) => {
	await mockMe(page, 401, { error: 'unauthorized' });

	await page.goto('/app');

	await expect(page).toHaveURL(/\/login$/);
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
});

test('authenticated: /app renders the protected surface when /me returns the account', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockGraph(page, 200, GRAPH_BODY);

	await page.goto('/app');

	await expect(page.getByTestId('graph-view')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('user-id')).toContainText(USER_ID);
});

test('reload stays authenticated (cookie-based session, not localStorage)', async ({ page }) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockGraph(page, 200, GRAPH_BODY);

	await page.goto('/app');
	await expect(page.getByTestId('graph-view')).toBeVisible({ timeout: 10_000 });

	await page.reload();

	await expect(page.getByTestId('graph-view')).toBeVisible({ timeout: 10_000 });
});

test('logout invalidates the session and redirects to /login; a later /app visit is rejected', async ({
	page
}) => {
	let meOk = true;
	await page.route('**/api/me', (route) =>
		route.fulfill({
			status: meOk ? 200 : 401,
			contentType: 'application/json',
			body: JSON.stringify(meOk ? { user_id: USER_ID } : { error: 'unauthorized' })
		})
	);
	await mockGraph(page, 200, GRAPH_BODY);
	await page.route('**/api/auth/logout', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify({ logged_out: true })
		})
	);

	await page.goto('/app');
	await expect(page.getByTestId('graph-view')).toBeVisible({ timeout: 10_000 });

	await page.getByTestId('logout-button').click();

	await expect(page).toHaveURL(/\/login$/);
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });

	meOk = false;
	await page.goto('/app');
	await expect(page).toHaveURL(/\/login$/);
});
