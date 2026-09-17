<script lang="ts">
	import { useModalSub } from '../../primitives/modal/modal.svelte.js';
	import * as Dialog from '../../primitives/dialog/index.js';
	import * as Drawer from '../../primitives/drawer/index.js';
	import type { DialogDescriptionProps } from 'bits-ui';

	const modal = useModalSub();

	let { ref = $bindable(null), ...rest }: DialogDescriptionProps = $props();
</script>

{#if modal.view === 'desktop'}
	<Dialog.Description bind:ref {...rest} />
{:else}
	<Drawer.Description bind:ref {...rest} />
{/if}
