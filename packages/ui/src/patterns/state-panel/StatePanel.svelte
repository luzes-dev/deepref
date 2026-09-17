<script lang="ts">
	import type { Snippet } from 'svelte';
	import { Badge } from '../../primitives/badge';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import InboxIcon from '@lucide/svelte/icons/inbox';
	import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
	import { cn } from '../../internal/utils';

	type State = 'loading' | 'empty' | 'error' | 'degraded' | 'success';

	let {
		state,
		title,
		description,
		action,
		children,
		class: className = '',
		testId,
		status
	}: {
		state: State;
		title: string;
		description?: string;
		action?: Snippet;
		children?: Snippet;
		class?: string;
		testId?: string;
		status?: string;
	} = $props();

	const iconTone = $derived(
		{
			loading: 'text-primary',
			empty: 'text-muted-foreground',
			error: 'text-destructive',
			degraded: 'text-warning',
			success: 'text-success'
		}[state]
	);
</script>

<section
	class={cn(
		'flex min-h-48 flex-col items-center justify-center rounded-lg border border-border-subtle bg-muted/30 p-page text-center',
		className
	)}
	data-state-panel
	data-state={state}
	data-testid={testId}
	aria-live={state === 'loading' || state === 'error' || state === 'degraded'
		? 'polite'
		: undefined}
>
	{#if state === 'loading'}
		<LoaderCircleIcon class={cn('mb-3 animate-spin', iconTone)} aria-hidden="true" />
	{:else if state === 'empty'}
		<InboxIcon class={cn('mb-3', iconTone)} aria-hidden="true" />
	{:else if state === 'error' || state === 'degraded'}
		<CircleAlertIcon class={cn('mb-3', iconTone)} aria-hidden="true" />
	{:else}
		<CircleCheckIcon class={cn('mb-3', iconTone)} aria-hidden="true" />
	{/if}
	{#if status}
		<div class="flex flex-wrap items-center justify-center gap-2">
			<h2 class="text-base font-semibold text-foreground">{title}</h2>
			<Badge variant={state === 'error' ? 'destructive' : state === 'degraded' ? 'warning' : state === 'success' ? 'success' : 'secondary'}>{status}</Badge>
		</div>
	{:else}
		<h2 class="text-base font-semibold text-foreground">{title}</h2>
	{/if}
	{#if description}<p class="mt-1 max-w-md text-sm leading-relaxed text-muted-foreground">{description}</p>{/if}
	{#if action}<div class="mt-4">{@render action()}</div>{/if}
	{#if children}<div class="mt-4">{@render children()}</div>{/if}
</section>
