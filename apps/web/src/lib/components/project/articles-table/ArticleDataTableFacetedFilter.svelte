<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
</script>

<script lang="ts" generics="TData, TValue">
	import CheckIcon from '@lucide/svelte/icons/check';
	import CirclePlusIcon from '@lucide/svelte/icons/circle-plus';
	import type { Column } from '@tanstack/table-core';
	import { SvelteSet } from 'svelte/reactivity';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Command from '$lib/components/ui/command';
	import * as Popover from '$lib/components/ui/popover';
	import { Separator } from '$lib/components/ui/separator';
	import { cn } from '$lib/utils.js';

	type FilterOption = {
		label: string;
		value: string;
	};

	let {
		column,
		title,
		options
	}: {
		column: Column<TData, TValue>;
		title: string;
		options: FilterOption[];
	} = $props();

	const facets = $derived(column.getFacetedUniqueValues());
	const selectedValues = $derived(new SvelteSet((column.getFilterValue() as string[]) ?? []));
</script>

<Popover.Root>
	<Popover.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="outline" size="sm" class="h-8 border-dashed">
				<CirclePlusIcon data-icon="inline-start" />
				{title}
				{#if selectedValues.size > 0}
					<Separator orientation="vertical" class="mx-1.5 h-4" />
					<Badge variant="secondary" class="rounded-sm px-1 font-normal lg:hidden">
						{selectedValues.size}
					</Badge>
					<div class="hidden items-center gap-1 lg:flex">
						{#if selectedValues.size > 2}
							<Badge variant="secondary" class="rounded-sm px-1 font-normal">
								{selectedValues.size} selected
							</Badge>
						{:else}
							{#each options.filter( (option) => selectedValues.has(option.value) ) as option (option.value)}
								<Badge variant="secondary" class="rounded-sm px-1 font-normal">
									{option.label}
								</Badge>
							{/each}
						{/if}
					</div>
				{/if}
			</Button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content class="w-52 p-0" align="start">
		<Command.Root>
			<Command.Input placeholder={title} />
			<Command.List>
				<Command.Empty>No results found.</Command.Empty>
				<Command.Group>
					{#each options as option (option.value)}
						{@const isSelected = selectedValues.has(option.value)}
						<Command.Item
							data-checked={isSelected}
							onSelect={() => {
								if (isSelected) {
									selectedValues.delete(option.value);
								} else {
									selectedValues.add(option.value);
								}

								const filterValues = Array.from(selectedValues);
								column.setFilterValue(
									filterValues.length ? filterValues : undefined
								);
							}}
						>
							<div
								class={cn(
									'border-primary flex size-4 items-center justify-center rounded-sm border',
									isSelected
										? 'bg-primary text-primary-foreground'
										: 'opacity-50 [&_svg]:invisible'
								)}
							>
								<CheckIcon />
							</div>
							<span>{option.label}</span>
							{#if facets.get(option.value)}
								<span
									class="ml-auto flex size-4 items-center justify-center font-mono text-xs"
								>
									{facets.get(option.value)}
								</span>
							{/if}
						</Command.Item>
					{/each}
				</Command.Group>
				{#if selectedValues.size > 0}
					<Command.Separator />
					<Command.Group>
						<Command.Item
							onSelect={() => column.setFilterValue(undefined)}
							class="justify-center text-center"
						>
							Clear filters
						</Command.Item>
					</Command.Group>
				{/if}
			</Command.List>
		</Command.Root>
	</Popover.Content>
</Popover.Root>
