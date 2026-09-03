<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';

	let {
		label,
		value,
		detail,
		trend,
		tone = 'default',
		children,
		class: className = ''
	}: {
		label: string;
		value: string | number;
		detail?: string;
		trend?: string;
		tone?: 'default' | 'positive' | 'warning' | 'critical' | 'info';
		children?: Snippet;
		class?: string;
	} = $props();

	const accentClass = $derived(
		{
			default: 'bg-primary',
			positive: 'bg-success',
			warning: 'bg-warning',
			critical: 'bg-destructive',
			info: 'bg-info'
		}[tone]
	);
</script>

<article
	class={cn(
		'relative overflow-hidden rounded-lg bg-card p-section ring-1 ring-foreground/10',
		className
	)}
	data-metric-tile
	data-tone={tone}
>
	<div class={cn('absolute inset-y-0 left-0 w-1', accentClass)} aria-hidden="true"></div>
	<div class="pl-2">
		<p class="text-xs font-medium tracking-[0.08em] text-muted-foreground uppercase">{label}</p>
		<p class="mt-2 text-3xl font-semibold tracking-tight text-card-foreground tabular-nums">
			{value}
		</p>
		{#if detail || trend || children}
			<div
				class="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground"
			>
				{#if detail}<span>{detail}</span>{/if}
				{#if trend}<span class="font-medium text-foreground">{trend}</span>{/if}
				{#if children}{@render children()}{/if}
			</div>
		{/if}
	</div>
</article>
