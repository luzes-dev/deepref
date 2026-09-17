<script lang="ts" module>
	import { type VariantProps, tv } from 'tailwind-variants';

	export const bubbleVariants = tv({
		base: 'relative max-w-full rounded-2xl px-4 py-3 text-sm leading-relaxed transition-colors shadow-2xs',
		variants: {
			variant: {
				default: 'bg-primary text-primary-foreground [a]:underline',
				muted: 'bg-muted text-foreground border border-border/50',
				outline: 'border border-border bg-card text-card-foreground shadow-xs'
			}
		},
		defaultVariants: {
			variant: 'default'
		}
	});

	export type BubbleVariant = VariantProps<typeof bubbleVariants>['variant'];
</script>

<script lang="ts">
	import type { HTMLAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		class: className,
		variant = 'default',
		children,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		variant?: BubbleVariant;
	} = $props();
</script>

<div
	bind:this={ref}
	data-slot="bubble-root"
	data-variant={variant}
	class={cn(bubbleVariants({ variant }), className)}
	{...restProps}
>
	{@render children?.()}
</div>
