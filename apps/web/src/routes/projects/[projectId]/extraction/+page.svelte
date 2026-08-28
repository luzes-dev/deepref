<script lang="ts">
	import { page } from '$app/state';
	import ExtractionScreen from '$lib/features/extraction/components/ExtractionScreen.svelte';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<ExtractionScreen {projectId} />
