<script lang="ts">
	import DataView from './DataView.svelte';
	let { value, depth = 0 }: { value: unknown; depth?: number } = $props();

	function humanise(key: string): string {
		const spaced = key.replace(/_/g, ' ');
		return spaced.charAt(0).toUpperCase() + spaced.slice(1);
	}
	const entries = $derived(
		typeof value === 'object' && value !== null && !Array.isArray(value)
			? Object.entries(value as Record<string, unknown>)
			: []
	);
	const items = $derived(Array.isArray(value) ? value : []);
</script>

{#if value === null || value === undefined || value === ''}
	<span class="text-muted-foreground">nothing</span>
{:else if Array.isArray(value)}
	{#if items.length === 0}
		<span class="text-muted-foreground">an empty list</span>
	{:else if depth >= 3}
		<span class="text-muted-foreground">{items.length} items</span>
	{:else}
		<details open={depth === 0}>
			<summary class="cursor-pointer text-muted-foreground"
				>{items.length} {items.length === 1 ? 'item' : 'items'}</summary
			>
			<ol class="mt-1 flex flex-col gap-1 border-l border-border pl-2">
				{#each items.slice(0, 20) as item, index (index)}
					<li><DataView value={item} depth={depth + 1} /></li>
				{/each}
				{#if items.length > 20}
					<li class="text-muted-foreground">and {items.length - 20} more</li>
				{/if}
			</ol>
		</details>
	{/if}
{:else if typeof value === 'object'}
	{#if depth >= 3}
		<span class="text-muted-foreground">details</span>
	{:else}
		<dl class="flex flex-col gap-0.5">
			{#each entries as [key, item] (key)}
				<div class="flex flex-col">
					<dt class="text-3xs text-muted-foreground">{humanise(key)}</dt>
					<dd class="text-xs break-words"><DataView value={item} depth={depth + 1} /></dd>
				</div>
			{/each}
		</dl>
	{/if}
{:else if typeof value === 'boolean'}
	<span>{value ? 'Yes' : 'No'}</span>
{:else}
	<span class="break-words">{String(value)}</span>
{/if}
