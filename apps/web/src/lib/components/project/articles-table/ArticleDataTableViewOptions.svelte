<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData">
	import Settings2Icon from '@lucide/svelte/icons/settings-2';
	import type { Table } from '@tanstack/table-core';
	import { Button } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

	const columnLabels: Record<string, string> = {
		title: 'Article',
		type: 'Type',
		issued_year: 'Year',
		total_citations: 'Total',
		internal_citations: 'Internal',
		outbound_internal_references: 'Outbound',
		rank_score: 'Rank'
	};

	let { table }: { table: Table<TData> } = $props();
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="outline" size="sm" class="ml-auto hidden h-8 lg:flex">
				<Settings2Icon data-icon="inline-start" />
				View
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="end">
		<DropdownMenu.Group>
			<DropdownMenu.GroupHeading>Toggle columns</DropdownMenu.GroupHeading>
			<DropdownMenu.Separator />
			{#each table
				.getAllColumns()
				.filter((column) => typeof column.accessorFn !== 'undefined' && column.getCanHide()) as column (column.id)}
				<DropdownMenu.CheckboxItem
					bind:checked={
						() => column.getIsVisible(),
						(value) => column.toggleVisibility(Boolean(value))
					}
				>
					{columnLabels[column.id] ?? column.id}
				</DropdownMenu.CheckboxItem>
			{/each}
		</DropdownMenu.Group>
	</DropdownMenu.Content>
</DropdownMenu.Root>
