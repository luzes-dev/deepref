<script lang="ts">
	import { page } from '$app/state';
	import ProjectAssistant from '$lib/features/assistant/components/ProjectAssistant.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<ProjectAssistant {projectId} />
