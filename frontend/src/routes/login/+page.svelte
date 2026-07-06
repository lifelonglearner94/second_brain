<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { browserSupportsWebAuthn } from '@simplewebauthn/browser';
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api';
	import {
		registerPasskey,
		loginPasskey,
		recoverPasskey
	} from '$lib/auth/flow';
	import { session } from '$lib/state/session.svelte';

	let busy = $state<null | 'register' | 'login' | 'recover'>(null);
	let status = $state<string | null>(null);
	let error = $state<string | null>(null);
	let recoverMessage = $state<string | null>(null);

	const supported = browserSupportsWebAuthn();

	// Issue #74 introduced the `?invite=<token>` deep link; issue #79 adds the
	// out-of-band path — an invitee who receives a bare token needs a place to
	// paste it. Both entry paths converge on a single source of truth: the
	// text input inside the disclosure below. When the query param is present,
	// onMount pre-fills the input and opens the disclosure so the invitee can
	// see (and edit) the token they arrived with. Read on the client only:
	// the login page is prerendered, and `page.url.searchParams` is not
	// available during prerender.
	let inviteInput = $state('');
	let disclosureOpen = $state(false);

	onMount(() => {
		const queryInvite = page.url.searchParams.get('invite');
		if (queryInvite) {
			inviteInput = queryInvite;
			disclosureOpen = true;
		}
	});

	// The single value the register flow consumes and the label reads. Null
	// when empty so registerBegin posts `{ invite: null }` (the bootstrap
	// exception path — the first registration creates the admin with no token).
	let effectiveInviteToken = $derived(
		inviteInput.trim() ? inviteInput.trim() : null
	);

	async function onRegister() {
		busy = 'register';
		error = null;
		status = null;
		try {
			const { user_id } = await registerPasskey(
				apiClient,
				effectiveInviteToken
			);
			// Registration mints a session (the backend sets the cookie), so the
			// user is authenticated immediately — update session state and go to
			// the app rather than asking them to sign in again.
			session.setAuthenticated(user_id);
			await goto('/app');
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

<main class="page page-narrow login">
	<section class="login-card rise">
		<header class="login-head">
			<div class="brand-mark" aria-hidden="true">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.6"
				>
					<circle cx="6" cy="7" r="2.2" />
					<circle cx="18" cy="7" r="2.2" />
					<circle cx="12" cy="17" r="2.6" />
					<path d="M7.6 8.4 10.6 15" />
					<path d="M16.4 8.4 13.4 15" />
					<path d="M8.2 7 15.8 7" />
				</svg>
			</div>
			<div>
				<p class="eyebrow">Second Brain</p>
				<h1>Sign in</h1>
			</div>
		</header>
		<p class="tagline">Passkey-only authentication. No passwords.</p>

		{#if !supported}
			<p class="warn pill pill-warn" data-testid="webauthn-unsupported">
				This browser does not support passkeys (WebAuthn).
			</p>
		{/if}

		<form
			class="auth-form"
			data-testid="auth-form"
			onsubmit={(e) => e.preventDefault()}
		>
			<button
				type="button"
				class="btn btn-primary auth-action"
				data-testid="login-button"
				onclick={onLogin}
				disabled={busy !== null}
			>
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="1.8"
					aria-hidden="true"
				>
					<rect x="4" y="10" width="16" height="11" rx="2.5" />
					<path d="M8 10V7a4 4 0 1 1 8 0v3" />
				</svg>
				{busy === 'login' ? 'Signing in…' : 'Sign in with passkey'}
			</button>

		<details class="invite-disclosure" bind:open={disclosureOpen}>
			<summary data-testid="invite-disclosure-toggle">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					aria-hidden="true"
				>
					<path d="M9 18l6-6-6-6" />
				</svg>
				Have an invitation token?
			</summary>
			<div class="invite-input">
				<label class="sr-only" for="invite-token">Invitation token</label>
				<input
					id="invite-token"
					class="input"
					type="text"
					data-testid="invite-token-input"
					bind:value={inviteInput}
					placeholder="Paste the token your admin shared"
					autocomplete="off"
					spellcheck={false}
				/>
			</div>
		</details>

		<button
			type="button"
			class="btn auth-action"
			data-testid="register-button"
			onclick={onRegister}
			disabled={busy !== null}
		>
			<svg
				viewBox="0 0 24 24"
				fill="none"
				stroke="currentColor"
				stroke-width="1.8"
				aria-hidden="true"
			>
				<path d="M12 5v14M5 12h14" />
			</svg>
			{busy === 'register'
				? 'Registering…'
				: effectiveInviteToken
					? 'Register with invitation'
					: 'Register a passkey'}
		</button>

			<button
				type="button"
				class="btn btn-secondary auth-action"
				data-testid="recover-button"
				onclick={onRecover}
				disabled={busy !== null}
			>
				{busy === 'recover' ? 'Recovering…' : 'Recover with master passphrase'}
			</button>

			{#if status}
				<p class="status pill pill-info" data-testid="auth-status">
					{status}
				</p>
			{/if}
			{#if recoverMessage}
				<p class="status pill pill-info" data-testid="recover-message">
					{recoverMessage}
				</p>
			{/if}
			{#if error}
				<p class="error pill pill-danger" data-testid="auth-error">
					{error}
				</p>
			{/if}
		</form>

		<p class="back">
			<a href="/" data-testid="goto-home">
				<svg
					viewBox="0 0 24 24"
					fill="none"
					stroke="currentColor"
					stroke-width="2"
					aria-hidden="true"
				>
					<path d="M15 18l-6-6 6-6" />
				</svg>
				Back to home
			</a>
		</p>
	</section>
</main>

<style>
	.login {
		display: grid;
		place-items: center;
		min-block-size: 100dvh;
	}
	.login-card {
		width: 100%;
		display: grid;
		gap: var(--space-4);
		padding: var(--space-8);
		background: var(--surface-glass);
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-xl);
		box-shadow: var(--shadow-2);
		backdrop-filter: blur(14px) saturate(120%);
		-webkit-backdrop-filter: blur(14px) saturate(120%);
	}
	.login-head {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}
	.brand-mark {
		display: grid;
		place-items: center;
		inline-size: 2.5rem;
		block-size: 2.5rem;
		flex: 0 0 auto;
		color: var(--accent);
		background: var(--accent-soft);
		border: 1px solid var(--border-accent);
		border-radius: var(--radius-md);
		box-shadow: 0 0 24px -10px var(--accent-glow);
	}
	.brand-mark svg {
		inline-size: 1.4rem;
		block-size: 1.4rem;
	}
	.login-head h1 {
		font-size: var(--fs-22);
		font-weight: 600;
	}
	.tagline {
		color: var(--fg-muted);
		font-size: var(--fs-14);
		margin-block-end: var(--space-2);
	}
	.warn {
		padding: 0.5rem 0.8rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
	}
	.auth-form {
		display: grid;
		gap: var(--space-3);
		padding-block: var(--space-2);
	}
	.auth-action {
		min-block-size: 48px;
		justify-content: center;
		font-size: var(--fs-16);
	}
	.auth-action svg {
		inline-size: 1.15rem;
		block-size: 1.15rem;
	}
	.invite-disclosure {
		border: 1px solid var(--border-hairline);
		border-radius: var(--radius-md);
		background: var(--bg-sunken);
		overflow: hidden;
	}
	.invite-disclosure > summary {
		display: flex;
		align-items: center;
		gap: var(--space-2);
		padding: 0.55rem 0.8rem;
		cursor: pointer;
		font-size: var(--fs-14);
		color: var(--fg-muted);
		list-style: none;
		user-select: none;
	}
	.invite-disclosure > summary::-webkit-details-marker {
		display: none;
	}
	.invite-disclosure > summary svg {
		inline-size: 1rem;
		block-size: 1rem;
		transition: transform var(--dur-1) var(--ease);
	}
	.invite-disclosure[open] > summary svg {
		transform: rotate(90deg);
	}
	.invite-disclosure > summary:hover {
		color: var(--fg);
	}
	.invite-input {
		padding: 0 0.8rem 0.7rem;
	}
	.status,
	.error {
		padding: 0.5rem 0.8rem;
		font-size: var(--fs-13);
		text-transform: none;
		letter-spacing: normal;
		line-height: 1.4;
	}
	.back {
		margin-block-start: var(--space-2);
		text-align: center;
	}
	.back a {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		color: var(--fg-muted);
		font-size: var(--fs-13);
	}
	.back a:hover {
		color: var(--fg);
	}
	.back svg {
		inline-size: 1rem;
		block-size: 1rem;
	}
</style>
