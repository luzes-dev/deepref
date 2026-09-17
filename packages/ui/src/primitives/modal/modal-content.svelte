<script lang="ts">
	import { useModalSub } from '../../primitives/modal/modal.svelte.js';
	import * as Dialog from '../../primitives/dialog/index.js';
	import * as Drawer from '../../primitives/drawer/index.js';
	import type { DialogContentProps } from 'bits-ui';

	const modal = useModalSub();

	let {
		ref = $bindable(null),
		showCloseButton = true,
		children,
		...rest
	}: DialogContentProps & { showCloseButton?: boolean } = $props();
</script>

{#if modal.view === 'desktop'}
	<Dialog.Content bind:ref {showCloseButton} {...rest}>
		{@render children?.()}
	</Dialog.Content>
{:else}
	<Drawer.Content bind:ref {...rest}>
		{@render children?.()}
	</Drawer.Content>
{/if}
