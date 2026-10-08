<script lang="ts">
	import { page } from '$app/state';
	import AutomationBuilder from '$lib/features/automations/AutomationBuilder.svelte';

	const projectId = $derived(page.params.projectId ?? '');
	const workflowId = $derived(page.params.workflowId ?? '');
	const initialRunId = $derived(page.url.searchParams.get('run'));
</script>

{#key workflowId}
	<AutomationBuilder {projectId} {workflowId} {initialRunId} />
{/key}
