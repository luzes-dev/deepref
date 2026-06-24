<script lang="ts">
	import XIcon from '@lucide/svelte/icons/x';
	import { Button, type ButtonProps } from '$lib/components/ui/button/index.js';

	let {
		class: className = 'absolute top-4 right-4',
		variant = 'ghost',
		size = 'icon-sm',
		...restProps
	}: ButtonProps = $props();
</script>

<Button {variant} {size} class={className} {...restProps}>
	<XIcon />
	<span class="sr-only">Close</span>
</Button>
