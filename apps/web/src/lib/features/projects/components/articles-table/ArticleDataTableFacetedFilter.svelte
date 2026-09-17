<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
	export { type FilterOption } from '@deepref/ui';
</script>

<script lang="ts" generics="TData, TValue">
	import { DataTableFacetedFilter } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';

	let { ...props }: ComponentProps<typeof DataTableFacetedFilter<TData, TValue>> = $props();
</script>

<DataTableFacetedFilter {...props} />
