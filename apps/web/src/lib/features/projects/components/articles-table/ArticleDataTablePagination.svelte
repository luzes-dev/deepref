<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData">
	import { DataTablePagination } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';

	let { ...props }: ComponentProps<typeof DataTablePagination<TData>> = $props();
</script>

<DataTablePagination {...props} />
