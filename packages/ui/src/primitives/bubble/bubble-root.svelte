<script lang="ts" module>
	import { type VariantProps, tv } from 'tailwind-variants';

	export const bubbleVariants = tv({
		base: 'relative max-w-full rounded-2xl leading-relaxed transition-colors shadow-2xs',
		variants: {
			variant: {
				default: 'bg-primary text-primary-foreground [a]:underline',
				muted: 'bg-muted text-foreground border border-border/50',
				outline: 'border border-border bg-card text-card-foreground shadow-xs'
			},
			size: {
				default: 'px-4 py-3 text-sm',
				compact: 'px-4 py-2.5 text-sm'
			}
		},
		defaultVariants: {
			variant: 'default',
			size: 'default'
		}
	});

	export type BubbleVariant = VariantProps<typeof bubbleVariants>['variant'];
	export type BubbleSize = VariantProps<typeof bubbleVariants>['size'];
</script>

<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		variant = 'default',
		size = 'default',
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		variant?: BubbleVariant;
		size?: BubbleSize;
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="bubble-root"
	data-variant={variant}
	class={cn(bubbleVariants({ variant, size }), className)}
	{...restProps}
>
	{@render children?.()}
</div>
