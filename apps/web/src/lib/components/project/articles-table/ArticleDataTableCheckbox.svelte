<script lang="ts">
	import type { ComponentProps } from 'svelte';
	import { Checkbox } from '$lib/components/ui/checkbox';

	let {
		checked = false,
		indeterminate = false,
		onCheckedChange = (value) => (checked = Boolean(value)),
		...restProps
	}: ComponentProps<typeof Checkbox> = $props();
</script>

<Checkbox bind:checked={() => checked, onCheckedChange} {indeterminate} {...restProps} />
