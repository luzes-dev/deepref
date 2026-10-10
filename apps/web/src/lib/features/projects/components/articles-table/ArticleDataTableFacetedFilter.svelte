<script lang="ts" module>
	export { type FilterOption } from '@deepref/ui';
</script>

<script lang="ts">
	import { DataTableFacetedFilter } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';
	import type { ReportDto } from '#lib/api/generated/models/index.js';

	let { ...props }: ComponentProps<typeof DataTableFacetedFilter<ReportDto, unknown>> = $props();
</script>

<DataTableFacetedFilter {...props} />
