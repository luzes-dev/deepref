<script lang="ts">
	import StudiesScreen from '$lib/features/studies/components/StudiesScreen.svelte';
	import { page } from '$app/state';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<StudiesScreen {projectId} />
