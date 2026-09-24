<script lang="ts" module>
	import { type VariantProps, tv } from 'tailwind-variants';

	export const badgeVariants = tv({
		base: 'max-w-full gap-1 rounded-sm border border-transparent font-medium transition-colors has-data-[icon=inline-end]:pr-1.5 has-data-[icon=inline-start]:pl-1.5 [&>svg]:size-3! focus-visible:border-ring focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive group/badge inline-flex w-fit shrink items-center justify-center break-words whitespace-normal focus-visible:ring-0 [&>svg]:pointer-events-none',
		variants: {
			variant: {
				default: 'bg-primary text-primary-foreground [a]:hover:bg-primary/80',
				secondary: 'bg-secondary text-secondary-foreground [a]:hover:bg-secondary/80',
				destructive: 'border-destructive-border bg-destructive-surface text-destructive',
				success: 'border-success-border bg-success-surface text-success',
				warning: 'border-warning-border bg-warning-surface text-warning',
				info: 'border-info-border bg-info-surface text-info',
				outline:
					'border-border text-foreground [a]:hover:bg-muted [a]:hover:text-muted-foreground',
				ghost: 'hover:bg-muted hover:text-muted-foreground dark:hover:bg-muted/50',
				link: 'text-primary underline-offset-4 hover:underline'
			},
			size: {
				default: 'min-h-5 px-2 py-0.5 text-xs',
				sm: 'min-h-4 px-1.5 py-0.5 text-2xs',
				xs: 'min-h-3.5 px-1 py-0 text-3xs'
			}
		},
		defaultVariants: {
			variant: 'default',
			size: 'default'
		}
	});

	export type BadgeVariant = VariantProps<typeof badgeVariants>['variant'];
	export type BadgeSize = VariantProps<typeof badgeVariants>['size'];
</script>

<script lang="ts">
	import type { HTMLAnchorAttributes } from 'svelte/elements';
	import { cn, type WithElementRef } from '../../internal/utils.js';

	let {
		ref = $bindable(null),
		href,
		class: className,
		variant = 'default',
		size = 'default',
		children,
		...restProps
	}: WithElementRef<HTMLAnchorAttributes, HTMLAnchorElement | HTMLSpanElement> & {
		variant?: BadgeVariant;
		size?: BadgeSize;
	} = $props();
</script>

<svelte:element
	this={href ? 'a' : 'span'}
	bind:this={ref}
	data-slot="badge"
	{href}
	class={cn(badgeVariants({ variant, size }), className)}
	{...restProps}
>
	{@render children?.()}
</svelte:element>
