<script lang="ts">
	import { page } from '$app/state';
	import ScreeningViewSwitch from '$lib/features/ai-autonomy/components/ScreeningViewSwitch.svelte';
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

<ScreeningViewSwitch {projectId} stage="title_abstract">
	<ScreeningFocus {projectId} />
</ScreeningViewSwitch>
