<script lang="ts">
	import { DataTableCheckbox } from '@deepref/ui';
	import type { ComponentProps } from 'svelte';

	let { ...props }: ComponentProps<typeof DataTableCheckbox> = $props();
</script>

<DataTableCheckbox {...props} />
