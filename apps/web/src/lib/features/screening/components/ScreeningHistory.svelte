<script lang="ts">
	import type { ScreeningHistoryItemDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { cn } from '$lib/utils';
	import { History, Undo2 } from '@lucide/svelte';

	let {
		items
	}: {
		items: ScreeningHistoryItemDto[];
	} = $props();

	function statusLabel(value: string) {
		return value.replaceAll('_', ' ');
	}
</script>

<Card.Root class="border-primary/15">
	<Card.Header class="gap-2 border-b border-border/60 pb-4">
		<div class="flex items-center gap-2">
			<span
				class="flex size-8 items-center justify-center rounded-lg bg-muted text-muted-foreground"
			>
				<History aria-hidden="true" />
			</span>
			<Card.Title>Decision history</Card.Title>
		</div>
		<Card.Description
			>Append-only record of decisions, reversals, and protocol versions.</Card.Description
		>
	</Card.Header>
	<Card.Content class="pt-0">
		{#if items.length === 0}
			<Empty.Root class="min-h-32 border-dashed p-6">
				<Empty.Media variant="icon"><History aria-hidden="true" /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No decision has been recorded for this report.</Empty.Title>
				</Empty.Header>
			</Empty.Root>
		{:else}
			<ol class="flex flex-col" aria-label="Auditable screening history">
				{#each items as item (item.id)}
					<li class="relative flex gap-3 border-l border-border/70 pb-5 pl-4 last:pb-0">
						<span
							class={cn(
								'absolute top-1 -left-[5px] size-2 rounded-full',
								item.event_kind === 'undo' ? 'bg-warning' : 'bg-primary'
							)}
							aria-hidden="true"
						></span>
						<div class="min-w-0 flex-1 rounded-lg border bg-muted/20 p-3 text-sm">
							<div class="flex flex-wrap items-center gap-2">
								<Badge
									variant={item.event_kind === 'undo' ? 'outline' : 'secondary'}
								>
									{#if item.event_kind === 'undo'}<Undo2
											data-icon="inline-start"
										/>{/if}
									{item.event_kind === 'undo'
										? 'Undo'
										: (item.decision ?? 'Decision')}
								</Badge>
								<Badge variant="outline">{item.stage.replace('_', ' ')}</Badge>
							</div>
							<div
								class="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground"
							>
								<time datetime={item.created_at}
									>{new Date(item.created_at).toLocaleString()}</time
								>
								<span aria-hidden="true">·</span>
								<span>{item.actor_kind} · {item.actor_id}</span>
								<span aria-hidden="true">·</span>
								<span>Protocol {item.protocol_version_id.slice(0, 8)}</span>
							</div>
							<div class="mt-3 grid gap-2 text-xs sm:grid-cols-2">
								<div>
									<span class="text-muted-foreground">Before</span>
									<br />
									{statusLabel(item.previous_title_abstract_status)} title ·
									{statusLabel(item.previous_full_text_status)} full text
								</div>
								<div>
									<span class="text-muted-foreground">After</span>
									<br />
									{statusLabel(item.result_title_abstract_status)} title ·
									{statusLabel(item.result_full_text_status)} full text
								</div>
							</div>
							{#if item.undoes_event_id || item.supersedes_event_id}
								<p class="mt-2 text-xs text-muted-foreground">
									{#if item.undoes_event_id}Undoes {item.undoes_event_id.slice(
											0,
											8
										)}{/if}
									{#if item.supersedes_event_id}
										· Supersedes {item.supersedes_event_id.slice(0, 8)}{/if}
								</p>
							{/if}
							{#if item.notes}<p class="mt-2 text-xs text-muted-foreground italic">
									{item.notes}
								</p>{/if}
						</div>
					</li>
				{/each}
			</ol>
		{/if}
	</Card.Content>
</Card.Root>
