import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApiClient } from '../../src/lib/api/client';
import type {
	AuthenticationResponseJSON,
	PublicKeyCredentialCreationOptionsJSON,
	PublicKeyCredentialRequestOptionsJSON,
	RegistrationResponseJSON
} from '@simplewebauthn/browser';

function okResponse(body: unknown, status = 200): Response {
	return new Response(JSON.stringify(body), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}

const CREATION_OPTIONS: PublicKeyCredentialCreationOptionsJSON = {
	rp: { id: 'localhost', name: 'Second Brain' },
	user: {
		id: '00000000-0000-0000-0000-000000000001',
		name: 'me',
		displayName: 'me'
	},
	challenge: 'AAAA',
	pubKeyCredParams: [{ type: 'public-key', alg: -7 }]
};

const REQUEST_OPTIONS: PublicKeyCredentialRequestOptionsJSON = {
	challenge: 'BBBB',
	rpId: 'localhost',
	userVerification: 'required'
};

const REGISTRATION_RESPONSE: RegistrationResponseJSON = {
	id: 'cred-id',
	rawId: 'cred-id',
	response: {
		clientDataJSON: 'client-data',
		attestationObject: 'attestation'
	},
	clientExtensionResults: {},
	type: 'public-key'
};

const AUTH_RESPONSE: AuthenticationResponseJSON = {
	id: 'cred-id',
	rawId: 'cred-id',
	response: {
		clientDataJSON: 'client-data',
		authenticatorData: 'auth-data',
		signature: 'sig'
	},
	clientExtensionResults: {},
	type: 'public-key'
};

describe('apiClient — passkey auth surface against backend #2', () => {
	let fetchMock: ReturnType<typeof vi.fn<typeof fetch>>;

	beforeEach(() => {
		fetchMock = vi.fn<typeof fetch>();
	});

	it('POST /auth/register/begin posts the optional invite token as JSON', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				challenge: { publicKey: CREATION_OPTIONS },
				state: 'state-1'
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		const begin = await api.registerBegin('invite-token-abc');
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/auth/register/begin');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
		expect(JSON.parse(init?.body as string)).toEqual({ invite: 'invite-token-abc' });
		expect(begin).toEqual({
			challenge: { publicKey: CREATION_OPTIONS },
			state: 'state-1'
		});
	});

	it('POST /auth/register/begin posts { invite: null } when no token is supplied', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				challenge: { publicKey: CREATION_OPTIONS },
				state: 'state-1'
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		await api.registerBegin();
		const [, init] = fetchMock.mock.calls[0];
		expect(JSON.parse(init?.body as string)).toEqual({ invite: null });
	});

	it('POST /auth/register/finish posts the credential + state as JSON', async () => {
		fetchMock.mockResolvedValue(
			okResponse({ registered: true, user_id: '00000000-0000-0000-0000-000000000001' })
		);
		const api = createApiClient({ fetch: fetchMock });
		const ok = await api.registerFinish({
			credential: REGISTRATION_RESPONSE,
			state: 'state-1'
		});
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/auth/register/finish');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
		expect(init?.headers).toMatchObject({ 'content-type': 'application/json' });
		expect(JSON.parse(init?.body as string)).toEqual({
			credential: REGISTRATION_RESPONSE,
			state: 'state-1'
		});
		expect(ok).toEqual({
			registered: true,
			user_id: '00000000-0000-0000-0000-000000000001'
		});
	});

	it('POST /auth/login/begin returns the request challenge + opaque state token', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				challenge: { publicKey: REQUEST_OPTIONS },
				state: 'state-2'
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		const begin = await api.loginBegin();
		expect(fetchMock.mock.calls[0][0]).toBe('/api/auth/login/begin');
		expect(begin).toEqual({
			challenge: { publicKey: REQUEST_OPTIONS },
			state: 'state-2'
		});
	});

	it('POST /auth/login/finish posts the assertion + state and returns user_id', async () => {
		fetchMock.mockResolvedValue(
			okResponse({ user_id: '00000000-0000-0000-0000-000000000001' })
		);
		const api = createApiClient({ fetch: fetchMock });
		const ok = await api.loginFinish({
			credential: AUTH_RESPONSE,
			state: 'state-2'
		});
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/auth/login/finish');
		expect(JSON.parse(init?.body as string)).toEqual({
			credential: AUTH_RESPONSE,
			state: 'state-2'
		});
		expect(ok).toEqual({ user_id: '00000000-0000-0000-0000-000000000001' });
	});

	it('GET /me returns the account id for a credentialed request', async () => {
		fetchMock.mockResolvedValue(
			okResponse({ user_id: '00000000-0000-0000-0000-000000000001' })
		);
		const api = createApiClient({ fetch: fetchMock });
		const me = await api.getMe();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/me');
		expect(init?.method).toBeUndefined();
		expect(init?.credentials).toBe('include');
		expect(me).toEqual({ user_id: '00000000-0000-0000-0000-000000000001' });
	});

	it('POST /auth/logout invalidates the session (credentialed)', async () => {
		fetchMock.mockResolvedValue(okResponse({ logged_out: true }));
		const api = createApiClient({ fetch: fetchMock });
		const ok = await api.logout();
		const [url, init] = fetchMock.mock.calls[0];
		expect(url).toBe('/api/auth/logout');
		expect(init?.method).toBe('POST');
		expect(init?.credentials).toBe('include');
		expect(ok).toEqual({ logged_out: true });
	});

	it('POST /auth/recover returns the stubbed recovery seam', async () => {
		fetchMock.mockResolvedValue(
			okResponse({
				error: 'recovery_not_implemented',
				message:
					'Master-passphrase recovery is a documented seam; not yet implemented.'
			})
		);
		const api = createApiClient({ fetch: fetchMock });
		const res = await api.recover();
		expect(fetchMock.mock.calls[0][0]).toBe('/api/auth/recover');
		expect(res.error).toBe('recovery_not_implemented');
	});

	it('throws on a non-2xx response (401 from /me when no session)', async () => {
		fetchMock.mockResolvedValue(new Response('unauthorized', { status: 401 }));
		const api = createApiClient({ fetch: fetchMock });
		await expect(api.getMe()).rejects.toThrow(/401/);
	});
});
