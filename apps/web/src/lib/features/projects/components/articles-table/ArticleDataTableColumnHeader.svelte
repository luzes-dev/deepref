<script lang="ts" module>
	type TData = unknown;
	type TValue = unknown;
</script>

<script lang="ts" generics="TData, TValue">
	import { DataTableColumnHeader } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';

	let { ...props }: ComponentProps<typeof DataTableColumnHeader<TData, TValue>> = $props();
</script>

<DataTableColumnHeader {...props} />
