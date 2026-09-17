<script lang="ts">
	import { page } from '$app/state';
	import ProjectChatAssistant from '$lib/features/assistant/components/ProjectChatAssistant.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<ProjectChatAssistant {projectId} />
