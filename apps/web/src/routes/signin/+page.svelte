<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { onMount } from 'svelte';

	// AuthGate shows the sign-in screen to anyone without a session. A person who
	// does have one (or a deployment without sign-in) landing here goes home.
	onMount(() => {
		void goto(resolve('/'), { replaceState: true });
	});
</script>
