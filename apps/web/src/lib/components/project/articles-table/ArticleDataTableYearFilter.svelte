<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
</script>

<script lang="ts" generics="TData, TValue">
	import CirclePlusIcon from '@lucide/svelte/icons/circle-plus';
	import type { Column } from '@tanstack/table-core';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Popover from '$lib/components/ui/popover';
	import { Separator } from '$lib/components/ui/separator';
	import { Slider } from '$lib/components/ui/slider';

	type YearRange = [number, number];

	let {
		column,
		minYear,
		maxYear
	}: {
		column: Column<TData, TValue>;
		minYear: number;
		maxYear: number;
	} = $props();

	const filterValue = $derived(column.getFilterValue() as YearRange | undefined);
	const selectedRange = $derived<YearRange>([
		filterValue?.[0] ?? minYear,
		filterValue?.[1] ?? maxYear
	]);
	const isFiltered = $derived(Boolean(filterValue));
	const rangeLabel = $derived(
		selectedRange[0] === selectedRange[1]
			? String(selectedRange[0])
			: `${selectedRange[0]}-${selectedRange[1]}`
	);

	function applyRange(value: number[]) {
		const nextMin = Math.min(value[0] ?? minYear, value[1] ?? maxYear);
		const nextMax = Math.max(value[0] ?? minYear, value[1] ?? maxYear);

		column.setFilterValue(
			nextMin === minYear && nextMax === maxYear ? undefined : [nextMin, nextMax]
		);
	}
</script>

<Popover.Root>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="outline" size="sm" class="h-8 border-dashed">
				<CirclePlusIcon data-icon="inline-start" />
				Year
				{#if isFiltered}
					<Separator orientation="vertical" class="mx-1.5 h-4" />
					<Badge variant="secondary" class="rounded-sm px-1 font-normal">
						{rangeLabel}
					</Badge>
				{/if}
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content class="w-64 p-4" align="start">
		<div class="flex flex-col gap-4">
			<div class="flex items-center justify-between gap-3 text-sm">
				<span class="font-medium">Year range</span>
				<span class="text-muted-foreground">{rangeLabel}</span>
			</div>
			<Slider
				type="multiple"
				value={selectedRange}
				min={minYear}
				max={maxYear}
				step={1}
				disabled={minYear === maxYear}
				onValueChange={applyRange}
			/>
			<div class="flex items-center justify-between text-xs text-muted-foreground">
				<span>{minYear}</span>
				<span>{maxYear}</span>
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
