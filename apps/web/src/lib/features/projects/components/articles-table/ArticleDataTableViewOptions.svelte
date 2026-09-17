<script lang="ts" module>
	type TData = unknown;
</script>

<script lang="ts" generics="TData">
	import { DataTableViewOptions } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';

	let { ...props }: ComponentProps<typeof DataTableViewOptions<TData>> = $props();
</script>

<DataTableViewOptions {...props} />
