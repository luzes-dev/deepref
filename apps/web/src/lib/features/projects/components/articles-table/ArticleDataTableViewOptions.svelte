<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData extends RowData">
	import { DataTableViewOptions } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';
	import type { RowData } from '@tanstack/svelte-table';

	let { ...props }: ComponentProps<typeof DataTableViewOptions<TData>> = $props();
</script>

<DataTableViewOptions {...props} />
