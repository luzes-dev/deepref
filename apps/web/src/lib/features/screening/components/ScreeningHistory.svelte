<script lang="ts">
	import type { ScreeningHistoryItemDto } from '#lib/api/generated/models/index.js';
	import { cn } from '#lib/utils.js';
	import { Undo2 } from '@lucide/svelte';
	import { describeHistoryItem } from '../history';

	let {
		items
	}: {
		items: ScreeningHistoryItemDto[];
	} = $props();

	const entries = $derived(items.map((item) => ({ item, ...describeHistoryItem(item) })));
</script>

<section class="flex flex-col gap-3" aria-label="Decision history">
	<h3 class="text-sm font-semibold">Decision history</h3>
	{#if items.length === 0}
		<p class="text-sm text-muted-foreground">No decision has been recorded for this report.</p>
	{:else}
		<ol class="flex flex-col" aria-label="Auditable screening history">
			{#each entries as { item, headline, meta, change } (item.id)}
				<li
					class="relative flex flex-col gap-0.5 border-l border-border pb-4 pl-4 text-sm last:pb-0"
				>
					<span
						class={cn(
							'absolute top-1.5 -left-0.75 size-1.5 rounded-full',
							item.event_kind === 'undo' ? 'bg-warning' : 'bg-primary'
						)}
						aria-hidden="true"
					></span>
					<span class="flex flex-wrap items-center gap-x-2 font-medium">
						{#if item.event_kind === 'undo'}<Undo2
								class="size-3.5"
								aria-hidden="true"
							/>{/if}
						{headline}
					</span>
					<span class="text-xs text-muted-foreground">
						<time datetime={item.created_at}>{meta}</time>
					</span>
					{#if change}<span class="text-xs text-muted-foreground">{change}</span>{/if}
					{#if item.notes}<p class="text-xs text-muted-foreground italic">
							{item.notes}
						</p>{/if}
				</li>
			{/each}
		</ol>
	{/if}
</section>
