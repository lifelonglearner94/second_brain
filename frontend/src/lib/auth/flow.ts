import {
	startAuthentication,
	startRegistration
} from '@simplewebauthn/browser';

import type {
	LoginBegin,
	LoginFinishBody,
	LoginOk,
	RecoverResponse,
	RegistrationBegin,
	RegistrationFinishBody
} from '$lib/api/client';

export type RegisterApi = {
	registerBegin(invite?: string | null): Promise<RegistrationBegin>;
	registerFinish(body: RegistrationFinishBody): Promise<{ registered: true; user_id: string }>;
};

export type LoginApi = {
	loginBegin(): Promise<LoginBegin>;
	loginFinish(body: LoginFinishBody): Promise<LoginOk>;
};

export type RecoverApi = {
	recover(): Promise<RecoverResponse>;
};

/**
 * Register a passkey. Issue #74: registration is invite-gated with a bootstrap
 * exception. Pass an admin-issued `invite` token; when the bootstrap exception
 * is open (zero users) the backend ignores the token, otherwise it is required
 * and must be valid + unconsumed. Registration mints a session (the backend
 * sets the cookie), so on success the caller is authenticated - the resolved
 * `user_id` is returned so the caller can update session state and navigate.
 */
export async function registerPasskey(
	api: RegisterApi,
	invite?: string | null
): Promise<{ user_id: string }> {
	const begin = await api.registerBegin(invite);
	const credential = await startRegistration({
		optionsJSON: begin.challenge.publicKey
	});
	const ok = await api.registerFinish({ credential, state: begin.state });
	return { user_id: ok.user_id };
}

export async function loginPasskey(api: LoginApi): Promise<LoginOk> {
	const begin = await api.loginBegin();
	const credential = await startAuthentication({
		optionsJSON: begin.challenge.publicKey
	});
	return api.loginFinish({ credential, state: begin.state });
}

export async function recoverPasskey(
	api: RecoverApi
): Promise<RecoverResponse> {
	return api.recover();
}
