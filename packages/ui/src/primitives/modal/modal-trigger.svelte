<script lang="ts">
	import { useModalSub } from '../../primitives/modal/modal.svelte.js';
	import * as Dialog from '../../primitives/dialog/index.js';
	import * as Drawer from '../../primitives/drawer/index.js';
	import type { DialogTriggerProps } from 'bits-ui';

	const modal = useModalSub();

	let { ref = $bindable(null), children, ...rest }: DialogTriggerProps = $props();
</script>

{#if modal.view === 'desktop'}
	<Dialog.Trigger bind:ref {...rest}>
		{@render children?.()}
	</Dialog.Trigger>
{:else}
	<Drawer.Trigger bind:ref {...rest}>
		{@render children?.()}
	</Drawer.Trigger>
{/if}
