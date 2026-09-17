<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
</script>

<script lang="ts" generics="TData, TValue">
	import CirclePlusIcon from '@lucide/svelte/icons/circle-plus';
	import type { Column } from '@tanstack/table-core';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import * as Popover from '@deepref/ui/popover';
	import { Separator } from '@deepref/ui/separator';
	import { Slider } from '@deepref/ui/slider';

	let {
		column,
		maxCitations
	}: {
		column: Column<TData, TValue>;
		maxCitations: number;
	} = $props();

	const filterValue = $derived((column.getFilterValue() as number | undefined) ?? 0);
	const isFiltered = $derived(filterValue > 0);

	function applyValue(value: number | undefined) {
		column.setFilterValue(value && value > 0 ? value : undefined);
	}
</script>

<Popover.Root>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="outline" size="sm" class="h-8 border-dashed">
				<CirclePlusIcon data-icon="inline-start" />
				Citations
				{#if isFiltered}
					<Separator orientation="vertical" class="mx-1.5 h-4" />
					<Badge variant="secondary" class="rounded-sm px-1 font-normal">
						Min {filterValue}
					</Badge>
				{/if}
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content class="w-64 p-4" align="start">
		<div class="flex flex-col gap-4">
			<div class="flex items-center justify-between gap-3 text-sm">
				<span class="font-medium">Min internal citations</span>
				<Badge variant="outline">Min {filterValue}</Badge>
			</div>
			<Slider
				type="single"
				value={filterValue}
				min={0}
				max={Math.max(1, maxCitations)}
				step={1}
				thumbLabel="Minimum internal citations"
				onValueChange={(val) => applyValue(val ? Number(val) : undefined)}
			/>
			<div class="flex items-center justify-between text-xs text-muted-foreground">
				<span>0</span>
				<span>{Math.max(1, maxCitations)}</span>
			</div>
			{#if isFiltered}
				<Button
					variant="ghost"
					size="sm"
					class="h-8"
					onclick={() => column.setFilterValue(undefined)}
				>
					Clear filter
				</Button>
			{/if}
		</div>
	</Popover.Content>
</Popover.Root>
