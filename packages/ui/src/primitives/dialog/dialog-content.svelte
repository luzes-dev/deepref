<script lang="ts">
	import { Dialog as DialogPrimitive } from 'bits-ui';
	import CloseButton from '../close-button/close-button.svelte';
	import DialogOverlay from './dialog-overlay.svelte';
	import DialogPortal from './dialog-portal.svelte';
	import type { Snippet } from 'svelte';
	import { cn, type WithoutChildrenOrChild } from '../../internal/utils.js';
	import type { ComponentProps } from 'svelte';

	let {
		ref = $bindable(null),
		class: className,
		portalProps,
		children,
		showCloseButton = true,
		...restProps
	}: WithoutChildrenOrChild<DialogPrimitive.ContentProps> & {
		portalProps?: WithoutChildrenOrChild<ComponentProps<typeof DialogPortal>>;
		children: Snippet;
		showCloseButton?: boolean;
	} = $props();
</script>

<DialogPortal {...portalProps}>
	<DialogOverlay />
	<DialogPrimitive.Content
		bind:ref
		data-slot="dialog-content"
		class={cn(
			'fixed top-1/2 left-1/2 z-50 grid max-h-[calc(100dvh-2rem)] w-full overflow-y-auto overscroll-contain max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-section rounded-lg bg-popover p-6 text-sm text-popover-foreground border border-border shadow-overlay duration-[var(--motion-duration-panel)] outline-none sm:max-w-md data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95',
			className
		)}
		{...restProps}
	>
		{@render children?.()}
		{#if showCloseButton}
			<DialogPrimitive.Close data-slot="dialog-close">
				{#snippet child({ props })}
					<CloseButton {...props} />
				{/snippet}
			</DialogPrimitive.Close>
		{/if}
	</DialogPrimitive.Content>
</DialogPortal>
