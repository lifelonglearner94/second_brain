import { expect, test } from '@playwright/test';

const USER_ID = '00000000-0000-0000-0000-000000000001';

const GRAPH_BODY = {
	concepts: [{ id: 'c1', label: 'sleep', created_at: '2026-07-01T00:00:00Z' }],
	edges: [],
	partitions: [{ concept_id: 'c1', partition_id: 0 }]
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

function mockGraph(
	page: import('@playwright/test').Page,
	status: number,
	body: unknown
) {
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

	await expect(page.getByTestId('capture-section')).toBeVisible({
		timeout: 10_000
	});
	await expect(page.getByTestId('user-id')).toContainText(USER_ID);
});

test('reload stays authenticated (cookie-based session, not localStorage)', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockGraph(page, 200, GRAPH_BODY);

	await page.goto('/app');
	await expect(page.getByTestId('capture-section')).toBeVisible({
		timeout: 10_000
	});

	await page.reload();

	await expect(page.getByTestId('capture-section')).toBeVisible({
		timeout: 10_000
	});
});

test('logout invalidates the session and redirects to /login; a later /app visit is rejected', async ({
	page
}) => {
	let meOk = true;
	await page.route('**/api/me', (route) =>
		route.fulfill({
			status: meOk ? 200 : 401,
			contentType: 'application/json',
			body: JSON.stringify(
				meOk ? { user_id: USER_ID } : { error: 'unauthorized' }
			)
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
	await expect(page.getByTestId('capture-section')).toBeVisible({
		timeout: 10_000
	});

	await page.getByTestId('logout-button').click();

	await expect(page).toHaveURL(/\/login$/);
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });

	meOk = false;
	await page.goto('/app');
	await expect(page).toHaveURL(/\/login$/);
});

test('issue #74: visiting /login?invite=<token> affords "Register with invitation"', async ({
	page
}) => {
	await mockMe(page, 401, { error: 'unauthorized' });

	// The invite token is shared out-of-band as a query param. The registration
	// screen reads it and rebrands the register affordance so an invitee knows
	// they are consuming an invitation (not the bootstrap path).
	await page.goto('/login?invite=invite-token-abc');

	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('register-button')).toContainText(
		'Register with invitation'
	);
});

test('issue #74: visiting /login with no invite affords the plain "Register a passkey"', async ({
	page
}) => {
	await mockMe(page, 401, { error: 'unauthorized' });

	await page.goto('/login');

	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('register-button')).toContainText(
		'Register a passkey'
	);
});

test('issue #79: the deep link pre-fills the disclosure input and auto-opens it', async ({
	page
}) => {
	await mockMe(page, 401, { error: 'unauthorized' });

	await page.goto('/login?invite=invite-token-abc');

	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
	// The disclosure auto-opens and the input is pre-filled with the query token.
	await expect(page.getByTestId('invite-token-input')).toHaveValue(
		'invite-token-abc'
	);
	await expect(page.getByTestId('invite-disclosure-toggle')).toBeVisible();
	await expect(page.getByTestId('register-button')).toContainText(
		'Register with invitation'
	);
});

test('issue #79: opening the disclosure, pasting a token, and clicking register POSTs the token in the begin body', async ({
	page
}) => {
	let beginBody: { invite?: string | null } | null = null;
	await page.route('**/api/auth/register/begin', (route) => {
		beginBody = JSON.parse(route.request().postData() ?? '{}');
		// A minimal but valid PublicKeyCredentialCreationOptionsJSON so the
		// client hands off to the WebAuthn prompt (which has no authenticator
		// in CI and rejects - we only care that the begin request carried the
		// token, not that registration completes).
		return route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify({
				challenge: {
					publicKey: {
						rp: { id: '127.0.0.1', name: 'Second Brain' },
						user: { id: 'u1', name: 'me', displayName: 'me' },
						challenge: 'AAAA',
						pubKeyCredParams: [{ type: 'public-key', alg: -7 }]
					}
				},
				state: 'state-1'
			})
		});
	});

	await page.goto('/login');
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });
	await expect(page.getByTestId('register-button')).toContainText(
		'Register a passkey'
	);

	// Open the disclosure and paste a bare token an admin shared out-of-band.
	await page.getByTestId('invite-disclosure-toggle').click();
	await page.getByTestId('invite-token-input').fill('e2e-pasted-token');
	await expect(page.getByTestId('register-button')).toContainText(
		'Register with invitation'
	);

	await page.getByTestId('register-button').click();

	await expect.poll(() => beginBody).toEqual({ invite: 'e2e-pasted-token' });
});

test('issue #79: a 400 "an invitation token is required" from the backend is surfaced clearly in the error pill', async ({
	page
}) => {
	await page.route('**/api/auth/register/begin', (route) =>
		route.fulfill({
			status: 400,
			contentType: 'application/json',
			body: JSON.stringify({
				error: 'bad request: an invitation token is required to register'
			})
		})
	);

	await page.goto('/login');
	await expect(page.getByTestId('auth-form')).toBeVisible({ timeout: 10_000 });

	// No token present (bootstrap path closed → backend refuses with 400).
	await page.getByTestId('register-button').click();

	await expect(page.getByTestId('auth-error')).toContainText(
		/an invitation token is required to register/
	);
});
