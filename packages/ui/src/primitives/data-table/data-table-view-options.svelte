<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData extends RowData">
	import Settings2Icon from "@lucide/svelte/icons/settings-2";
	import type { Column, RowData } from "@tanstack/svelte-table";
	import { Button } from "../button/index.js";
	import * as DropdownMenu from "../dropdown-menu/index.js";

	type HideableColumn = Pick<
		Column<any, TData, unknown>,
		"id" | "accessorFn" | "getCanHide" | "getIsVisible" | "toggleVisibility"
	>;

	let {
		table,
		columnLabels = {},
	}: {
		table: { getAllColumns: () => HideableColumn[] };
		columnLabels?: Record<string, string>;
	} = $props();
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button
				{...props}
				variant="outline"
				size="sm"
				class="ml-auto hidden h-8 lg:flex"
			>
				<Settings2Icon data-icon="inline-start" />
				View
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content align="end">
		<DropdownMenu.Group>
			<DropdownMenu.GroupHeading>Toggle columns</DropdownMenu.GroupHeading
			>
			<DropdownMenu.Separator />
			{#each table
				.getAllColumns()
				.filter((column) => typeof column.accessorFn !== "undefined" && column.getCanHide()) as column (column.id)}
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
