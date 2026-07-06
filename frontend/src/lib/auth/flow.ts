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
	registerBegin(): Promise<RegistrationBegin>;
	registerFinish(body: RegistrationFinishBody): Promise<{ registered: true }>;
};

export type LoginApi = {
	loginBegin(): Promise<LoginBegin>;
	loginFinish(body: LoginFinishBody): Promise<LoginOk>;
};

export type RecoverApi = {
	recover(): Promise<RecoverResponse>;
};

export async function registerPasskey(api: RegisterApi): Promise<void> {
	const begin = await api.registerBegin();
	const credential = await startRegistration({
		optionsJSON: begin.challenge.publicKey
	});
	await api.registerFinish({ credential, state: begin.state });
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
