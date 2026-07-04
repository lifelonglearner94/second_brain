<script lang="ts">
	import { goto } from '$app/navigation';
	import { browserSupportsWebAuthn } from '@simplewebauthn/browser';
	import { apiClient } from '$lib/api';
	import { registerPasskey, loginPasskey, recoverPasskey } from '$lib/auth/flow';
	import { session } from '$lib/state/session.svelte';

	let busy = $state<null | 'register' | 'login' | 'recover'>(null);
	let status = $state<string | null>(null);
	let error = $state<string | null>(null);
	let recoverMessage = $state<string | null>(null);

	const supported = browserSupportsWebAuthn();

	async function onRegister() {
		busy = 'register';
		error = null;
		status = null;
		try {
			await registerPasskey(apiClient);
			status = 'Passkey registered — sign in with it below.';
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = null;
		}
	}

	async function onLogin() {
		busy = 'login';
		error = null;
		status = null;
		try {
			const ok = await loginPasskey(apiClient);
			session.setAuthenticated(ok.user_id);
			await goto('/app');
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = null;
		}
	}

	async function onRecover() {
		busy = 'recover';
		error = null;
		recoverMessage = null;
		try {
			const res = await recoverPasskey(apiClient);
			recoverMessage = res.message;
		} catch (e) {
			error = e instanceof Error ? e.message : String(e);
		} finally {
			busy = null;
		}
	}
</script>

<main>
	<header>
		<h1>Sign in</h1>
		<p class="tagline">Passkey-only authentication. No passwords.</p>
	</header>

	{#if !supported}
		<p data-testid="webauthn-unsupported" class="warn">
			This browser does not support passkeys (WebAuthn).
		</p>
	{/if}

	<form data-testid="auth-form" onsubmit={(e) => e.preventDefault()}>
		<button
			type="button"
			data-testid="register-button"
			onclick={onRegister}
			disabled={busy !== null}
		>
			{busy === 'register' ? 'Registering…' : 'Register a passkey'}
		</button>

		<button
			type="button"
			data-testid="login-button"
			onclick={onLogin}
			disabled={busy !== null}
		>
			{busy === 'login' ? 'Signing in…' : 'Sign in with passkey'}
		</button>

		<button
			type="button"
			data-testid="recover-button"
			class="secondary"
			onclick={onRecover}
			disabled={busy !== null}
		>
			{busy === 'recover' ? 'Recovering…' : 'Recover with master passphrase'}
		</button>

		{#if status}
			<p data-testid="auth-status" class="status">{status}</p>
		{/if}
		{#if recoverMessage}
			<p data-testid="recover-message" class="status">{recoverMessage}</p>
		{/if}
		{#if error}
			<p data-testid="auth-error" class="error">{error}</p>
		{/if}
	</form>

	<p><a href="/" data-testid="goto-home">Back to home</a></p>
</main>

<style>
	main {
		max-inline-size: 28rem;
		margin-inline: auto;
		padding: 2rem 1rem;
		font-family:
			system-ui,
			-apple-system,
			sans-serif;
		color: #e6e8ec;
		background: #0b0d12;
		min-block-size: 100vh;
		box-sizing: border-box;
	}
	h1 {
		margin: 0 0 0.25rem;
		font-size: clamp(1.5rem, 4vw, 2rem);
	}
	.tagline {
		margin: 0 0 1.5rem;
		color: #9aa3b2;
	}
	form {
		display: grid;
		gap: 0.75rem;
	}
	button {
		padding: 0.75rem 1rem;
		font-size: 1rem;
		border: 1px solid #2a2f3a;
		border-radius: 0.5rem;
		background: #1a1f2b;
		color: #e6e8ec;
		cursor: pointer;
	}
	button:disabled {
		opacity: 0.6;
		cursor: progress;
	}
	button.secondary {
		background: transparent;
		color: #9aa3b2;
	}
	.status {
		color: #7ab7ff;
		margin: 0;
	}
	.warn {
		color: #ffb077;
	}
	.error {
		color: #ff7a7a;
		margin: 0;
	}
	a {
		color: #7ab7ff;
	}
</style>
