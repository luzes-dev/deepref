<script lang="ts">
	import { page } from '$app/state';
	import AutomationCenter from '$lib/features/automations/components/AutomationCenter.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<AutomationCenter {projectId} />
