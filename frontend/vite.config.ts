import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit(), svelteTesting({ autoCleanup: false })],
	resolve: {
		conditions: ['browser']
	},
	test: {
		include: ['tests/unit/**/*.test.ts', 'src/**/*.test.ts'],
		environment: 'jsdom',
		setupFiles: ['tests/unit/setup-svelte.ts'],
		server: {
			deps: {
				inline: [
					'svelte',
					'@testing-library/svelte',
					'@testing-library/svelte-core'
				]
			}
		}
	}
});
