<script lang="ts">
	import { useModalSub } from '../../primitives/modal/modal.svelte.js';
	import * as Dialog from '../../primitives/dialog/index.js';
	import * as Drawer from '../../primitives/drawer/index.js';
	import type { DialogTitleProps } from 'bits-ui';

	const modal = useModalSub();

	let { ref = $bindable(null), ...rest }: DialogTitleProps = $props();
</script>

{#if modal.view === 'desktop'}
	<Dialog.Title bind:ref {...rest} />
{:else}
	<Drawer.Title bind:ref {...rest} />
{/if}
