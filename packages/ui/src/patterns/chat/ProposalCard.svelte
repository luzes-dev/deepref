<script lang="ts">
	import { Root as BubbleRoot } from '../../primitives/bubble/index.js';
	import { Badge } from '../../primitives/badge/index.js';
	import { Button } from '../../primitives/button/index.js';
	import { cn, type WithElementRef } from '../../internal/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';
	import SparklesIcon from '@lucide/svelte/icons/sparkles';
	import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';

	let {
		ref = $bindable(null),
		class: className,
		kind,
		summary,
		targetId,
		targetLabel = 'Target ID',
		reviewHref = '#',
		reviewLabel = 'Review in Queue',
		confidence,
		bubble = true,
		onReviewClick,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		kind: string;
		summary: string;
		targetId?: string;
		targetLabel?: string;
		reviewHref?: string;
		reviewLabel?: string;
		confidence?: number;
		bubble?: boolean;
		onReviewClick?: (e: MouseEvent) => void;
	} = $props();

	let formattedKind = $derived(
		kind
			.replace(/^propose_/, '')
			.replace(/_/g, ' ')
			.replace(/\b\w/g, (c) => c.toUpperCase())
	);

	let formattedConfidence = $derived(
		confidence !== undefined ? `${Math.round(confidence <= 1 ? confidence * 100 : confidence)}%` : null
	);
</script>

{#snippet cardInner()}
	<div class="flex w-full flex-col gap-3">
		<!-- Header: Kind & Confidence -->
		<div class="flex items-center justify-between gap-2 border-b border-border/50 pb-2.5">
			<div class="flex items-center gap-1.5">
				<span class="flex size-5 shrink-0 items-center justify-center rounded-sm bg-primary/10 text-primary">
					<SparklesIcon class="size-3.5" />
				</span>
				<Badge variant="outline" class="font-mono text-[11px] font-medium tracking-wide">
					{formattedKind}
				</Badge>
			</div>

			{#if formattedConfidence}
				<Badge variant="secondary" class="font-mono text-[10px]">
					{formattedConfidence} confidence
				</Badge>
			{/if}
		</div>

		<!-- Body: Summary & Target -->
		<div class="flex flex-col gap-2">
			<p data-slot="proposal-summary" class="text-sm leading-relaxed text-foreground">
				{summary}
			</p>

			{#if targetId}
				<div data-slot="proposal-target" class="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground">
					<span class="font-medium text-foreground/80">{targetLabel}:</span>
					<code class="rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-foreground select-all">
						{targetId}
					</code>
				</div>
			{/if}
		</div>

		<!-- Action Footer: Review in Queue -->
		<div class="flex items-center justify-end pt-1">
			<Button
				href={reviewHref}
				onclick={onReviewClick}
				size="sm"
				variant="default"
				data-slot="proposal-action"
				class="gap-1.5 text-xs shadow-2xs"
			>
				<span>{reviewLabel}</span>
				<ArrowRightIcon class="size-3.5" />
			</Button>
		</div>
	</div>
{/snippet}

{#if bubble}
	<BubbleRoot
		bind:ref
		variant="outline"
		data-slot="proposal-card"
		class={cn('w-full border-border bg-card text-card-foreground shadow-xs', className)}
		{...restProps}
	>
		{@render cardInner()}
	</BubbleRoot>
{:else}
	<div
		bind:this={ref}
		data-slot="proposal-card"
		class={cn('w-full rounded-2xl border border-border bg-card px-4 py-3 text-card-foreground shadow-xs', className)}
		{...restProps}
	>
		{@render cardInner()}
	</div>
{/if}
