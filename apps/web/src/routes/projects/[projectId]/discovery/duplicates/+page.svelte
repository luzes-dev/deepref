<script lang="ts">
	import DeduplicationQueue from '$lib/features/deduplication/components/DeduplicationQueue.svelte';
	import { page } from '$app/state';

	const projectId = $derived(page.params.projectId ?? '');
</script>

<svelte:head>
	<title>Deduplication · DeepRef</title>
	<meta
		name="description"
		content="Review duplicate-record proposals and preserve source-record provenance in a DeepRef evidence workspace."
	/>
</svelte:head>

<DeduplicationQueue {projectId} />
