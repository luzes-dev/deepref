<script lang="ts">
	import { page } from '$app/state';
	import AutomationList from '#lib/features/automations/AutomationList.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<AutomationList {projectId} />
