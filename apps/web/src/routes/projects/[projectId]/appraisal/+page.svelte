<script lang="ts">
	import AppraisalScreen from '$lib/features/appraisal/components/AppraisalScreen.svelte';
	import { page } from '$app/state';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<AppraisalScreen {projectId} />
