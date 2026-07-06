// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, fireEvent, cleanup, waitFor } from '@testing-library/svelte';
import type {
	PublicKeyCredentialCreationOptionsJSON,
	RegistrationResponseJSON
} from '@simplewebauthn/browser';

// Hoisted stubs so the vi.mock factories (which are hoisted above imports)
// can reference them without TDZ violations.
const stubs = vi.hoisted(() => ({
	registerBegin: vi.fn(),
	registerFinish: vi.fn(),
	setAuthenticated: vi.fn(),
	goto: vi.fn(),
	startRegistration: vi.fn(),
	pageHolder: { url: new URL('http://localhost/login') }
}));

vi.mock('$app/navigation', () => ({ goto: stubs.goto }));
vi.mock('$app/state', () => ({ get page() { return stubs.pageHolder; } }));
vi.mock('$lib/api', () => ({
	apiClient: {
		registerBegin: stubs.registerBegin,
		registerFinish: stubs.registerFinish
	}
}));
vi.mock('$lib/state/session.svelte', () => ({
	session: { setAuthenticated: stubs.setAuthenticated }
}));
vi.mock('@simplewebauthn/browser', () => ({
	browserSupportsWebAuthn: () => true,
	startRegistration: stubs.startRegistration,
	startAuthentication: vi.fn()
}));

// Imported after the mocks above are registered.
import Login from '../../src/routes/login/+page.svelte';

const CREATION_OPTIONS: PublicKeyCredentialCreationOptionsJSON = {
	rp: { id: 'localhost', name: 'Second Brain' },
	user: { id: 'u1', name: 'me', displayName: 'me' },
	challenge: 'AAAA',
	pubKeyCredParams: [{ type: 'public-key', alg: -7 }]
};

const REGISTRATION_RESPONSE: RegistrationResponseJSON = {
	id: 'cred-id',
	rawId: 'cred-id',
	response: { clientDataJSON: 'cd', attestationObject: 'ao' },
	clientExtensionResults: {},
	type: 'public-key'
};

function setUrl(url: string): void {
	stubs.pageHolder.url = new URL(url);
}

describe('login page — invitation-token disclosure (issue #79)', () => {
	beforeEach(() => {
		cleanup();
		setUrl('http://localhost/login');
		stubs.registerBegin.mockResolvedValue({
			challenge: { publicKey: CREATION_OPTIONS },
			state: 'state-1'
		});
		stubs.registerFinish.mockResolvedValue({
			registered: true,
			user_id: '00000000-0000-0000-0000-000000000001'
		});
		stubs.startRegistration.mockResolvedValue(REGISTRATION_RESPONSE);
		stubs.setAuthenticated.mockReset();
		stubs.goto.mockReset();
		stubs.registerBegin.mockClear();
		stubs.registerFinish.mockClear();
		stubs.startRegistration.mockClear();
	});

	afterEach(() => {
		cleanup();
	});

	it('offers a collapsible "Have an invitation token?" disclosure with a token input (closed by default)', async () => {
		const { getByTestId } = render(Login);
		const toggle = getByTestId('invite-disclosure-toggle');
		const details = toggle.closest('details') as HTMLDetailsElement;
		expect(details).toBeTruthy();
		expect(details.open).toBe(false);
		expect(getByTestId('invite-token-input')).toBeTruthy();
	});

	it('pasting a token and clicking register threads the token through registerBegin', async () => {
		const { getByTestId } = render(Login);

		// Open the disclosure and paste a token.
		await fireEvent.click(getByTestId('invite-disclosure-toggle'));
		const input = getByTestId('invite-token-input') as HTMLInputElement;
		await fireEvent.input(input, { target: { value: 'pasted-token-xyz' } });

		// The register affordance rebrands to reflect the present token.
		await waitFor(() =>
			expect(
				getByTestId('register-button').textContent ?? ''
			).toContain('Register with invitation')
		);

		await fireEvent.click(getByTestId('register-button'));

		await waitFor(() =>
			expect(stubs.registerBegin).toHaveBeenCalledWith('pasted-token-xyz')
		);
		expect(stubs.registerFinish).toHaveBeenCalledTimes(1);
		expect(stubs.setAuthenticated).toHaveBeenCalledWith(
			'00000000-0000-0000-0000-000000000001'
		);
	});

	it('the ?invite=<token> query param pre-fills the input and auto-opens the disclosure', async () => {
		setUrl('http://localhost/login?invite=tok-from-admin');
		const { getByTestId } = render(Login);

		await waitFor(() => {
			const input = getByTestId('invite-token-input') as HTMLInputElement;
			expect(input.value).toBe('tok-from-admin');
		});
		const details = getByTestId('invite-disclosure-toggle').closest(
			'details'
		) as HTMLDetailsElement;
		expect(details.open).toBe(true);
		expect(getByTestId('register-button').textContent ?? '').toContain(
			'Register with invitation'
		);
	});

	it('clearing the input reverts the register label to "Register a passkey"', async () => {
		setUrl('http://localhost/login?invite=tok-from-admin');
		const { getByTestId } = render(Login);
		await waitFor(() =>
			expect(
				getByTestId('register-button').textContent ?? ''
			).toContain('Register with invitation')
		);

		const input = getByTestId('invite-token-input') as HTMLInputElement;
		await fireEvent.input(input, { target: { value: '' } });

		await waitFor(() =>
			expect(
				getByTestId('register-button').textContent ?? ''
			).toContain('Register a passkey')
		);
	});

	it('registering with no token calls registerBegin with null (bootstrap path)', async () => {
		const { getByTestId } = render(Login);
		expect(getByTestId('register-button').textContent ?? '').toContain(
			'Register a passkey'
		);
		await fireEvent.click(getByTestId('register-button'));
		await waitFor(() =>
			expect(stubs.registerBegin).toHaveBeenCalledWith(null)
		);
	});
});
