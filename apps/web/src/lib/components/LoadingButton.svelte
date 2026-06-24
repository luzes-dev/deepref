<script lang="ts" module>
	import type { MouseEventHandler } from 'svelte/elements';
	import type { ButtonProps as ButtonPrimitiveProps } from '$lib/components/ui/button';

	export type LoadingButtonProps = ButtonPrimitiveProps & {
		loading?: boolean;
		onClickPromise?: (
			e:
				| Parameters<MouseEventHandler<HTMLButtonElement>>[0]
				| Parameters<MouseEventHandler<HTMLAnchorElement>>[0]
		) => Promise<void>;
	};

	export type Size = 'default' | 'xs' | 'sm' | 'lg';

	export const sizeMap = {
		default: {
			icon: 'icon',
			normal: 'default'
		},
		xs: {
			icon: 'icon-xs',
			normal: 'xs'
		},
		sm: {
			icon: 'icon-sm',
			normal: 'sm'
		},
		lg: {
			icon: 'icon-lg',
			normal: 'lg'
		}
	} as const;

	export { type ButtonSize, type ButtonVariant } from '$lib/components/ui/button';
	export type ButtonProps = LoadingButtonProps;
</script>

<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';

	let {
		ref = $bindable(null),
		loading: loadingProp = false,
		onClickPromise,
		onclick,
		disabled,
		children,
		...restProps
	}: LoadingButtonProps = $props();

	let pending = $state(false);

	const loading = $derived(loadingProp || pending);
</script>

<Button
	bind:ref
	disabled={loading || disabled}
	onclick={async (e) => {
		onclick?.(e as never);

		if (onClickPromise) {
			pending = true;
			try {
				await onClickPromise(e);
			} finally {
				pending = false;
			}
		}
	}}
	{...restProps}
>
	{#if loading}
		<Spinner data-icon="inline-start" />
	{/if}
	{@render children?.()}
</Button>
