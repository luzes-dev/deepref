<script lang="ts">
	import { DataTableColumnHeader } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';
	import type { ReportDto } from '#lib/api/generated/models/index.js';

	let { ...props }: ComponentProps<typeof DataTableColumnHeader<ReportDto, unknown>> = $props();
</script>

<DataTableColumnHeader {...props} />
