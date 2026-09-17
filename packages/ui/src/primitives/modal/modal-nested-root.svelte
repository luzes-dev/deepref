<script lang="ts">
	import * as Dialog from '../../primitives/dialog/index.js';
	import * as Drawer from '../../primitives/drawer/index.js';
	import type { DialogRootProps } from 'bits-ui';
	import { useModal } from '../../primitives/modal/modal.svelte.js';

	let { open = $bindable(false), children, ...rest }: DialogRootProps = $props();

	const modal = useModal();
</script>

{#if modal.view === 'desktop'}
	<Dialog.Root bind:open {...rest}>
		{@render children?.()}
	</Dialog.Root>
{:else}
	<Drawer.NestedRoot bind:open {...rest}>
		{@render children?.()}
	</Drawer.NestedRoot>
{/if}
