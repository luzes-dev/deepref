<script lang="ts">
	import { page } from '$app/state';
	import ActivityFeed from '#lib/features/ai-autonomy/components/ActivityFeed.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<svelte:head>
	<title>AI activity · DeepRef</title>
	<meta
		name="description"
		content="What the AI, automations and the assistant did in this project, with undo."
	/>
</svelte:head>

<ActivityFeed {projectId} />
