<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { session } from '$lib/state/session.svelte';

	let { children } = $props();

	onMount(() => {
		if (session.status === 'unknown') {
			session.refresh();
		}
	});
</script>

{@render children()}
