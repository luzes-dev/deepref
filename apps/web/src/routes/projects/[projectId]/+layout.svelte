<script lang="ts">
	import type { Snippet } from 'svelte';
	import ProjectWorkspace from '$lib/features/projects/components/ProjectWorkspace.svelte';

	let { children }: { children: Snippet } = $props();
</script>

<ProjectWorkspace {children} />
