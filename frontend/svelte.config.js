import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

const config = {
	preprocess: vitePreprocess(),
	kit: {
		adapter: adapter({
			pages: 'build',
			assets: 'build',
			precompress: false,
			strict: false
		}),
		prerender: {
			entries: ['*', '/login', '/app', '/app/admin/logs']
		},
		serviceWorker: {
			register: true
		}
	}
};

export default config;
