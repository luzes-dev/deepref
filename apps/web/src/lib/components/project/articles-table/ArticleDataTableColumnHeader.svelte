<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
</script>

<script lang="ts" generics="TData, TValue">
	import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
	import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
	import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';
	import type { Column } from '@tanstack/table-core';
	import { Button } from '$lib/components/ui/button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';
	import { cn } from '$lib/utils.js';

	type Props = {
		column: Column<TData, TValue>;
		title: string;
		class?: string;
	};

	let { column, title, class: className }: Props = $props();
</script>

{#if !column.getCanSort()}
	<div class={className}>{title}</div>
{:else}
	<div class={cn('flex items-center', className)}>
		<DropdownMenu.Root>
			<DropdownMenu.Trigger>
				{#snippet child({ props })}
					<Button
						{...props}
						variant="ghost"
						size="sm"
						class="data-[state=open]:bg-accent -ml-3 h-8"
					>
						<span>{title}</span>
						{#if column.getIsSorted() === 'desc'}
							<ArrowDownIcon data-icon="inline-end" />
						{:else if column.getIsSorted() === 'asc'}
							<ArrowUpIcon data-icon="inline-end" />
						{:else}
							<ChevronsUpDownIcon data-icon="inline-end" />
						{/if}
					</Button>
				{/snippet}
			</DropdownMenu.Trigger>
			<DropdownMenu.Content align="start">
				<DropdownMenu.Group>
					<DropdownMenu.Item onclick={() => column.toggleSorting(false)}>
						<ArrowUpIcon />
						Asc
					</DropdownMenu.Item>
					<DropdownMenu.Item onclick={() => column.toggleSorting(true)}>
						<ArrowDownIcon />
						Desc
					</DropdownMenu.Item>
					{#if column.getCanHide()}
						<DropdownMenu.Separator />
						<DropdownMenu.Item onclick={() => column.toggleVisibility(false)}>
							<EyeOffIcon />
							Hide
						</DropdownMenu.Item>
					{/if}
				</DropdownMenu.Group>
			</DropdownMenu.Content>
		</DropdownMenu.Root>
	</div>
{/if}
