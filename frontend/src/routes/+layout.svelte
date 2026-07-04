<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { session } from '$lib/auth/session';

	let { children } = $props();

	onMount(() => {
		if (session.status === 'unknown') {
			session.refresh();
		}
	});
</script>

{@render children()}
