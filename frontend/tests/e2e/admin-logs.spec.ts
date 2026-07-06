import { expect, test } from '@playwright/test';

const USER_ID = '00000000-0000-0000-0000-000000000001';

const LOGS_BODY = {
	logs: [
		{
			timestamp: 1_700_000_000,
			level: 'ERROR',
			target: 'gemini_client',
			message: 'generation failed',
			fields: { status: 503, retries: 3 }
		},
		{
			timestamp: 1_700_000_010,
			level: 'WARN',
			target: 'gemini_client',
			message: 'retrying',
			fields: { attempt: 1 }
		},
		{
			timestamp: 1_700_000_020,
			level: 'INFO',
			target: 'ingest',
			message: 'braindump accepted',
			fields: { id: 'b1' }
		}
	],
	count: 3,
	capacity: 1_000
};

function mockMe(
	page: import('@playwright/test').Page,
	status: number,
	body: unknown
) {
	return page.route('**/api/me', (route) =>
		route.fulfill({
			status,
			contentType: 'application/json',
			body: JSON.stringify(body)
		})
	);
}

function mockLogs(
	page: import('@playwright/test').Page,
	status: number,
	body: unknown
) {
	return page.route('**/api/admin/logs**', (route) =>
		route.fulfill({
			status,
			contentType: 'application/json',
			body: JSON.stringify(body)
		})
	);
}

test('unauthenticated: visiting /app/admin/logs redirects to /login (auth-gated by /app guard)', async ({
	page
}) => {
	await mockMe(page, 401, { error: 'unauthorized' });

	await page.goto('/app/admin/logs');

	await expect(page).toHaveURL(/\/login$/);
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
});

test('authenticated: the admin tab fetches and renders structured logs from GET /admin/logs', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockLogs(page, 200, LOGS_BODY);

	await page.goto('/app/admin/logs');

	await expect(page.getByTestId('admin-logs-list')).toBeVisible({
		timeout: 10_000
	});
	await expect(page.getByTestId('admin-logs-count')).toContainText('3');
	await expect(page.getByTestId('admin-logs-capacity')).toContainText('1000');
	await expect(page.getByTestId('admin-log-row')).toHaveCount(3);
	await expect(page.getByTestId('admin-log-message').first()).toContainText(
		'generation failed'
	);
});

test('the admin tab is hidden from primary nav and revealed by a non-obvious gesture (5 taps on the title)', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockLogs(page, 200, LOGS_BODY);

	await page.goto('/app');

	// Not in primary nav. app-title (the header) confirms the app shell loaded;
	// admin-link must be absent until the 5-tap gesture reveals it.
	await expect(page.getByTestId('app-title')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('admin-link')).toHaveCount(0);

	// Five taps on the title reveals the hidden admin entry.
	for (let i = 0; i < 5; i++) {
		await page.getByTestId('app-title').click();
	}
	await expect(page.getByTestId('admin-link')).toBeVisible({ timeout: 5_000 });

	await page.getByTestId('admin-link').click();

	await expect(page).toHaveURL(/\/app\/admin\/logs$/);
	await expect(page.getByTestId('admin-logs-list')).toBeVisible({
		timeout: 10_000
	});
});

test('level filter narrows the rendered list to the selected level', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockLogs(page, 200, LOGS_BODY);

	await page.goto('/app/admin/logs');
	await expect(page.getByTestId('admin-logs-list')).toBeVisible({
		timeout: 10_000
	});
	await expect(page.getByTestId('admin-log-row')).toHaveCount(3);

	await page.getByTestId('admin-logs-filter-WARN').click();
	await expect(page.getByTestId('admin-log-row')).toHaveCount(1);
	await expect(page.getByTestId('admin-log-message')).toContainText('retrying');

	await page.getByTestId('admin-logs-filter-ERROR').click();
	await expect(page.getByTestId('admin-log-row')).toHaveCount(1);
	await expect(page.getByTestId('admin-log-message')).toContainText(
		'generation failed'
	);

	await page.getByTestId('admin-logs-filter-all').click();
	await expect(page.getByTestId('admin-log-row')).toHaveCount(3);
});

test('text search narrows the rendered list across message, target, and structured fields', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockLogs(page, 200, LOGS_BODY);

	await page.goto('/app/admin/logs');
	await expect(page.getByTestId('admin-logs-list')).toBeVisible({
		timeout: 10_000
	});

	await page.getByTestId('admin-logs-search').fill('503');
	await expect(page.getByTestId('admin-log-row')).toHaveCount(1);
	await expect(page.getByTestId('admin-log-message')).toContainText(
		'generation failed'
	);

	await page.getByTestId('admin-logs-search').fill('ingest');
	await expect(page.getByTestId('admin-log-row')).toHaveCount(1);

	await page.getByTestId('admin-logs-search').fill('');
	await expect(page.getByTestId('admin-log-row')).toHaveCount(3);
});
