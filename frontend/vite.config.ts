import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	plugins: [sveltekit()],
	resolve: process.env.VITEST
		? { conditions: ['browser', 'module', 'import', 'default'] }
		: undefined,
	test: {
		include: ['tests/unit/**/*.test.ts'],
		environment: 'jsdom',
		server: {
			deps: {
				inline: ['svelte', '@testing-library/svelte', '@testing-library/svelte-core']
			}
		}
	}
});
