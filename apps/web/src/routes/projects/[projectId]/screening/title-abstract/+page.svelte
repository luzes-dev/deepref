<script lang="ts">
	import { page } from '$app/state';
	import ScreeningFocus from '$lib/features/screening/components/ScreeningFocus.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<svelte:head>
	<title>Title & abstract screening · DeepRef</title>
	<meta
		name="description"
		content="Protocol-grounded title and abstract screening for a DeepRef evidence workspace."
	/>
</svelte:head>

<ScreeningFocus {projectId} />
