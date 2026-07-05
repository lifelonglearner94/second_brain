import { describe, it, expect, vi, beforeEach } from 'vitest';
import { startRegistration, startAuthentication } from '@simplewebauthn/browser';
import type {
	AuthenticationResponseJSON,
	PublicKeyCredentialCreationOptionsJSON,
	PublicKeyCredentialRequestOptionsJSON,
	RegistrationResponseJSON
} from '@simplewebauthn/browser';

import {
	registerPasskey,
	loginPasskey,
	recoverPasskey,
	type RegisterApi,
	type LoginApi,
	type RecoverApi
} from '../../src/lib/auth/flow';
import type {
	LoginBegin,
	LoginFinishBody,
	LoginOk,
	RecoverResponse,
	RegistrationBegin,
	RegistrationFinishBody
} from '../../src/lib/api/client';

vi.mock('@simplewebauthn/browser', () => ({
	startRegistration: vi.fn(),
	startAuthentication: vi.fn()
}));

const CREATION_OPTIONS: PublicKeyCredentialCreationOptionsJSON = {
	rp: { id: 'localhost', name: 'Second Brain' },
	user: { id: 'u1', name: 'me', displayName: 'me' },
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
	response: { clientDataJSON: 'cd', attestationObject: 'ao' },
	clientExtensionResults: {},
	type: 'public-key'
};

const AUTH_RESPONSE: AuthenticationResponseJSON = {
	id: 'cred-id',
	rawId: 'cred-id',
	response: { clientDataJSON: 'cd', authenticatorData: 'ad', signature: 'sig' },
	clientExtensionResults: {},
	type: 'public-key'
};

function flowApiStub() {
	return {
		registerBegin: vi.fn<RegisterApi['registerBegin']>(),
		registerFinish: vi.fn<RegisterApi['registerFinish']>(),
		loginBegin: vi.fn<LoginApi['loginBegin']>(),
		loginFinish: vi.fn<LoginApi['loginFinish']>(),
		recover: vi.fn<RecoverApi['recover']>()
	};
}

const REGISTRATION_BEGIN: RegistrationBegin = {
	challenge: { publicKey: CREATION_OPTIONS },
	state: 'state-1'
};
const LOGIN_BEGIN: LoginBegin = { challenge: { publicKey: REQUEST_OPTIONS }, state: 'state-2' };
const REGISTRATION_FINISH_OK = { registered: true } as const;
const LOGIN_OK: LoginOk = { user_id: '00000000-0000-0000-0000-000000000001' };
const RECOVER_RES: RecoverResponse = {
	error: 'recovery_not_implemented',
	message: 'Master-passphrase recovery is a documented seam; not yet implemented.'
};

describe('registerPasskey — the begin→WebAuthn→finish orchestration', () => {
	beforeEach(() => vi.clearAllMocks());

	it('forwards the begin challenge to startRegistration and posts the credential + state to finish', async () => {
		vi.mocked(startRegistration).mockResolvedValue(REGISTRATION_RESPONSE);
		const api = flowApiStub();
		api.registerBegin.mockResolvedValue(REGISTRATION_BEGIN);
		api.registerFinish.mockResolvedValue(REGISTRATION_FINISH_OK);
		await registerPasskey(api);
		expect(api.registerBegin).toHaveBeenCalledOnce();
		expect(startRegistration).toHaveBeenCalledWith({ optionsJSON: CREATION_OPTIONS });
		expect(api.registerFinish).toHaveBeenCalledWith({
			credential: REGISTRATION_RESPONSE,
			state: 'state-1'
		} satisfies RegistrationFinishBody);
	});

	it('does not call finish when the user cancels the authenticator prompt (error propagates)', async () => {
		vi.mocked(startRegistration).mockRejectedValue(new Error('user cancelled'));
		const api = flowApiStub();
		api.registerBegin.mockResolvedValue(REGISTRATION_BEGIN);
		await expect(registerPasskey(api)).rejects.toThrow('user cancelled');
		expect(api.registerFinish).not.toHaveBeenCalled();
	});
});

describe('loginPasskey — the begin→WebAuthn→finish orchestration', () => {
	beforeEach(() => vi.clearAllMocks());

	it('forwards the begin challenge to startAuthentication and posts the assertion + state, returning user_id', async () => {
		vi.mocked(startAuthentication).mockResolvedValue(AUTH_RESPONSE);
		const api = flowApiStub();
		api.loginBegin.mockResolvedValue(LOGIN_BEGIN);
		api.loginFinish.mockResolvedValue(LOGIN_OK);
		const ok = await loginPasskey(api);
		expect(api.loginBegin).toHaveBeenCalledOnce();
		expect(startAuthentication).toHaveBeenCalledWith({ optionsJSON: REQUEST_OPTIONS });
		expect(api.loginFinish).toHaveBeenCalledWith({
			credential: AUTH_RESPONSE,
			state: 'state-2'
		} satisfies LoginFinishBody);
		expect(ok).toEqual(LOGIN_OK);
	});

	it('does not call finish when authentication is cancelled', async () => {
		vi.mocked(startAuthentication).mockRejectedValue(new Error('cancelled'));
		const api = flowApiStub();
		api.loginBegin.mockResolvedValue(LOGIN_BEGIN);
		await expect(loginPasskey(api)).rejects.toThrow('cancelled');
		expect(api.loginFinish).not.toHaveBeenCalled();
	});
});

describe('recoverPasskey — the master-passphrase recovery seam', () => {
	it('delegates to the API recover endpoint and surfaces the stubbed response', async () => {
		const api = flowApiStub();
		api.recover.mockResolvedValue(RECOVER_RES);
		const res = await recoverPasskey(api);
		expect(api.recover).toHaveBeenCalledOnce();
		expect(res.error).toBe('recovery_not_implemented');
	});
});
