<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		testId,
		maxWidth = 'default',
		scrollable,
		title,
		description,
		class: className = '',
		containerClass = '',
		header,
		toolbar,
		children,
		...restProps
	}: {
		testId?: string;
		maxWidth?: 'default' | 'wide' | 'narrow' | 'full';
		scrollable?: boolean;
		title?: string;
		description?: string;
		class?: string;
		containerClass?: string;
		header?: Snippet;
		toolbar?: Snippet;
		children: Snippet;
		[key: string]: unknown;
	} = $props();

	const maxWidthMap = {
		default: 'max-w-[1440px]',
		wide: 'max-w-[1536px]',
		narrow: 'max-w-4xl',
		full: 'max-w-none'
	};
</script>

<div
	class={cn(
		'flex min-h-full w-full flex-1 flex-col bg-background text-foreground focus:outline-none',
		scrollable === true && 'h-full overflow-auto',
		scrollable === false && 'h-full overflow-hidden',
		className
	)}
	tabindex="-1"
	data-testid={testId}
	{...restProps}
>
	{#if title}
		<div class="sr-only">
			<h1>{title}</h1>
			{#if description}<p>{description}</p>{/if}
		</div>
	{/if}

	<div
		class={cn(
			'mx-auto flex w-full flex-1 flex-col gap-6 p-4 sm:p-6 lg:p-8',
			maxWidthMap[maxWidth],
			containerClass
		)}
	>
		{#if header}
			{@render header()}
		{/if}

		{#if toolbar}
			{@render toolbar()}
		{/if}

		{@render children()}
	</div>
</div>
