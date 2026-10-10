<script lang="ts">
	import { page } from '$app/state';
	import ScreeningViewSwitch from '#lib/features/ai-autonomy/components/ScreeningViewSwitch.svelte';
	import FullTextScreening from '#lib/features/full-text/components/FullTextScreening.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<svelte:head>
	<title>Full-text screening · DeepRef</title>
	<meta
		name="description"
		content="Review PDF evidence and screen included reports at full text."
	/>
</svelte:head>

<ScreeningViewSwitch {projectId} stage="full_text">
	<FullTextScreening {projectId} />
</ScreeningViewSwitch>
