<script lang="ts" module>
	import { type VariantProps, tv } from 'tailwind-variants';

	export const markerVariants = tv({
		base: 'inline-flex items-center gap-2 rounded-full border border-border/60 bg-muted/60 px-3 py-1 text-xs text-muted-foreground backdrop-blur-xs transition-colors select-none',
		variants: {
			variant: {
				default: 'animate-pulse',
				pulse: 'animate-pulse',
				shimmer: 'shimmer animate-pulse relative overflow-hidden'
			}
		},
		defaultVariants: {
			variant: 'default'
		}
	});

	export type MarkerVariant = VariantProps<typeof markerVariants>['variant'];
</script>

<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		variant = 'default',
		role = 'status',
		'aria-live': ariaLive = 'polite',
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		variant?: MarkerVariant;
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="marker-root"
	data-variant={variant}
	{role}
	aria-live={ariaLive}
	class={cn(markerVariants({ variant }), className)}
	{...restProps}
>
	{@render children?.()}
</div>
