import { expect, test } from '@playwright/test';

const USER_ID = '00000000-0000-0000-0000-000000000001';

const IPHONE_UA =
	'Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Mobile/15E148 Safari/604.1';

const GRAPH_BODY = {
	concepts: [
		{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' },
		{ id: 'c2', label: 'melatonin', created_at: '2026-07-02T00:00:00Z' }
	],
	edges: [
		{
			id: 'e1',
			source_concept_id: 'c1',
			target_concept_id: 'c2',
			original_type: 'affects',
			current_type: 'affects',
			created_at: '2026-07-02T00:00:00Z'
		}
	],
	partitions: [
		{ concept_id: 'c1', partition_id: 0 },
		{ concept_id: 'c2', partition_id: 1 }
	]
};

function mockAuth(page: import('@playwright/test').Page) {
	return page.route('**/api/me', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify({ user_id: USER_ID })
		})
	);
}

test('Spatial View-Graph: fetches the Global Topology Snapshot and renders behind the auth guard', async ({
	page
}) => {
	await mockAuth(page);
	await page.route('**/api/graph', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(GRAPH_BODY)
		})
	);

	await page.goto('/app/graph');

	await expect(page.getByTestId('graph-view')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('graph-ready')).toBeVisible({
		timeout: 20_000
	});
});

test('Viewport State: the selected node is restored on reload so the PWA feels native (no amnesia)', async ({
	page
}) => {
	await mockAuth(page);
	await page.route('**/api/graph', (route) =>
		route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(GRAPH_BODY)
		})
	);

	await page.addInitScript(() => {
		localStorage.setItem(
			'sb.viewport-state',
			JSON.stringify({
				cameraX: 0,
				cameraY: 0,
				cameraZ: 300,
				zoom: 1,
				selectedNodeId: 'c1'
			})
		);
	});

	await page.goto('/app/graph');

	await expect(page.getByTestId('graph-ready')).toBeVisible({
		timeout: 20_000
	});
	await expect(page.getByTestId('selected-node-label')).toContainText('sleep', {
		timeout: 10_000
	});
});

test('Frozen Graph: falls back to the IDB cache when the backend is unreachable (ADR-0005)', async ({
	page
}) => {
	await mockAuth(page);

	let graphOk = true;
	await page.route('**/api/graph', (route) =>
		route.fulfill({
			status: graphOk ? 200 : 500,
			contentType: 'application/json',
			body: JSON.stringify(graphOk ? GRAPH_BODY : { error: 'unavailable' })
		})
	);

	await page.goto('/app/graph');
	await expect(page.getByTestId('graph-ready')).toBeVisible({
		timeout: 20_000
	});

	graphOk = false;
	await page.reload();

	const offline = page.getByTestId('graph-offline');
	await expect(offline).toBeVisible({ timeout: 15_000 });
	await expect(offline).toContainText(/Offline.*as of/);
});

test.describe('2D WebGL fallback (capability detection picks sigma.js on iOS)', () => {
	test.use({
		userAgent: IPHONE_UA,
		hasTouch: true,
		isMobile: true,
		viewport: { width: 390, height: 844 }
	});

	test('the Spatial View-Graph renders via the 2D sigma.js path when capability detection falls back from 3D', async ({
		page
	}) => {
		await mockAuth(page);
		await page.route('**/api/graph', (route) =>
			route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify(GRAPH_BODY)
			})
		);

		await page.goto('/app/graph');

		await expect(page.getByTestId('graph-view')).toBeVisible({
			timeout: 10_000
		});
		await expect(page.getByTestId('graph-ready')).toBeVisible({
			timeout: 20_000
		});
	});

	test('node selection is preserved in the 2D fallback (Viewport State restores the selected concept on reload)', async ({
		page
	}) => {
		await mockAuth(page);
		await page.route('**/api/graph', (route) =>
			route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify(GRAPH_BODY)
			})
		);

		await page.addInitScript(() => {
			localStorage.setItem(
				'sb.viewport-state',
				JSON.stringify({
					cameraX: 0,
					cameraY: 0,
					cameraZ: 300,
					zoom: 1,
					selectedNodeId: 'c1'
				})
			);
		});

		await page.goto('/app/graph');
		await expect(page.getByTestId('graph-ready')).toBeVisible({
			timeout: 20_000
		});
		await expect(page.getByTestId('selected-node-label')).toContainText(
			'sleep',
			{ timeout: 10_000 }
		);
	});
});
