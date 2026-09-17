<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '../../internal/utils';

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
</script>

<article
	class={cn('min-w-0 border-t border-subtle py-section data-[tone=positive]:border-success-border data-[tone=warning]:border-warning-border data-[tone=critical]:border-destructive-border data-[tone=info]:border-info-border', className)}
	data-metric-tile
	data-tone={tone}
>
	<div class="min-w-0">
		<p class="text-sm text-muted-foreground">{label}</p>
		<p class="mt-2 text-2xl break-words font-semibold tracking-tight text-foreground tabular-nums">
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
