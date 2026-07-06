import { expect, test } from '@playwright/test';

const USER_ID = '00000000-0000-0000-0000-000000000001';

const PENDING_INVITE = {
	id: 7,
	token: 'pending-token-abc123',
	created_by_user_id: USER_ID,
	status: 'pending',
	created_at: 1_700_000_000,
	consumed_at: null,
	consumed_by_user_id: null,
	consumed_by_display_name: null
};

const CONSUMED_INVITE = {
	id: 6,
	token: 'consumed-token-xyz',
	created_by_user_id: USER_ID,
	status: 'consumed',
	created_at: 1_699_999_000,
	consumed_at: 1_700_000_500,
	consumed_by_user_id: '00000000-0000-0000-0000-000000000002',
	consumed_by_display_name: 'user_b'
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

function mockInvitesList(
	page: import('@playwright/test').Page,
	body: unknown
) {
	return page.route('**/api/admin/invites', (route) => {
		if (route.request().method() === 'GET') {
			return route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify(body)
			});
		}
		return route.continue();
	});
}

// Single handler that answers both the GET list and the POST mint — Playwright
// dispatches the last-registered matching route first, and `route.continue()`
// does not reliably chain to earlier handlers, so combining the two methods
// in one handler is the robust shape (mirrors admin-logs.spec.ts).
function mockInvitesListAndMint(
	page: import('@playwright/test').Page,
	listBody: unknown,
	mintBody: unknown
) {
	return page.route('**/api/admin/invites', (route) => {
		if (route.request().method() === 'POST') {
			return route.fulfill({
				status: 200,
				contentType: 'application/json',
				body: JSON.stringify(mintBody)
			});
		}
		return route.fulfill({
			status: 200,
			contentType: 'application/json',
			body: JSON.stringify(listBody)
		});
	});
}

test('issue #78: after minting an invite, a "Copy invite link" affordance is visible and its data-attribute contains /login?invite=', async ({
	page,
	context
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockInvitesListAndMint(page, { invitations: [] }, PENDING_INVITE);

	// Grant clipboard permissions so the copy action does not fall to the
	// catch branch (which would clear the copied state), and so the readback
	// assertion can verify what was actually copied.
	await context.grantPermissions(['clipboard-write', 'clipboard-read']);

	await page.goto('/app/admin/invites');
	await expect(page.getByTestId('admin-invites-mint')).toBeVisible({
		timeout: 10_000
	});

	await page.getByTestId('admin-invites-mint').click();

	// The just-minted card shows a Copy invite link affordance.
	const mintedCopyLink = page.getByTestId('admin-invites-copy-link');
	await expect(mintedCopyLink).toBeVisible({ timeout: 10_000 });
	await expect(mintedCopyLink).toContainText('Copy invite link');

	// Its data-attribute carries the deep link the admin shares out-of-band.
	const linkAttr = await mintedCopyLink.getAttribute('data-invite-link');
	expect(linkAttr).toContain('/login?invite=');
	expect(linkAttr).toContain(PENDING_INVITE.token);

	// Clicking copies the deep link to the clipboard and flips the label.
	await mintedCopyLink.click();
	await expect(mintedCopyLink).toContainText('Copied');
	const clipboardText = await page.evaluate(() =>
		navigator.clipboard.readText()
	);
	expect(clipboardText).toContain('/login?invite=');
	expect(clipboardText).toContain(PENDING_INVITE.token);
});

test('issue #78: each pending row in the invitations list offers a "Copy invite link" affordance; consumed rows do not', async ({
	page
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockInvitesList(page, {
		invitations: [PENDING_INVITE, CONSUMED_INVITE]
	});

	await page.goto('/app/admin/invites');
	await expect(page.getByTestId('admin-invites-list')).toBeVisible({
		timeout: 10_000
	});

	const copyLinkButtons = page.getByTestId('admin-invite-copy-link');
	await expect(copyLinkButtons).toHaveCount(1);

	// The one visible copy-link button belongs to the pending row and carries
	// the deep link in its data-attribute.
	const linkAttr = await copyLinkButtons.first().getAttribute('data-invite-link');
	expect(linkAttr).toContain('/login?invite=');
	expect(linkAttr).toContain(PENDING_INVITE.token);
});

test('issue #78: copy-link and copy-token show independent copied-feedback (clicking one does not flip the other)', async ({
	page,
	context
}) => {
	await mockMe(page, 200, { user_id: USER_ID });
	await mockInvitesListAndMint(page, { invitations: [] }, PENDING_INVITE);
	await context.grantPermissions(['clipboard-write']);

	await page.goto('/app/admin/invites');
	await page.getByTestId('admin-invites-mint').click();

	const copyToken = page.getByTestId('admin-invites-copy');
	const copyLink = page.getByTestId('admin-invites-copy-link');

	await expect(copyToken).toBeVisible({ timeout: 10_000 });
	await expect(copyLink).toBeVisible({ timeout: 10_000 });

	// Click Copy invite link → it flips to "Copied", Copy token stays "Copy token".
	await copyLink.click();
	await expect(copyLink).toContainText('Copied');
	await expect(copyToken).toContainText('Copy token');

	// Click Copy token → it flips to "Copied" too; both now show "Copied"
	// (independent state — neither resets the other).
	await copyToken.click();
	await expect(copyToken).toContainText('Copied');
	await expect(copyLink).toContainText('Copied');
});
