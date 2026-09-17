<script lang="ts">
	import type { Snippet } from 'svelte';

	let { children }: { children?: Snippet } = $props();
</script>

<div
	class="workspace-page h-full min-h-0 overflow-auto overscroll-contain"
	data-testid="workspace-scroll"
>
	{#if children}
		{@render children()}
	{/if}
</div>
