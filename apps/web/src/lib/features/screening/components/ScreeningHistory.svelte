<script lang="ts">
	import type { ScreeningHistoryItemDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';

	let {
		items
	}: {
		items: ScreeningHistoryItemDto[];
	} = $props();

	function statusLabel(value: string) {
		return value.replaceAll('_', ' ');
	}
</script>

<Card.Root>
	<Card.Header>
		<Card.Title>Decision history</Card.Title>
		<Card.Description
			>Append-only actions, snapshots, actors, and protocol versions.</Card.Description
		>
	</Card.Header>
	<Card.Content>
		{#if items.length === 0}
			<p class="text-sm text-muted-foreground">
				No decision has been recorded for this report.
			</p>
		{:else}
			<ol class="flex flex-col gap-4" aria-label="Auditable screening history">
				{#each items as item (item.id)}
					<li class="rounded-md border p-3 text-sm">
						<div class="flex flex-wrap items-center gap-2">
							<Badge variant={item.event_kind === 'undo' ? 'outline' : 'secondary'}>
								{item.event_kind === 'undo'
									? 'Undo'
									: (item.decision ?? 'Decision')}
							</Badge>
							<Badge variant="outline">{item.stage.replace('_', ' ')}</Badge>
							<span class="text-xs text-muted-foreground">
								{item.actor_kind} · {item.actor_id} · Protocol {item.protocol_version_id.slice(
									0,
									8
								)}
							</span>
						</div>
						<p class="mt-2 text-xs text-muted-foreground">
							{new Date(item.created_at).toLocaleString()}
						</p>
						<div class="mt-2 grid gap-2 text-xs sm:grid-cols-2">
							<div>
								<span class="font-medium">Before:</span>
								{statusLabel(item.previous_title_abstract_status)} title ·
								{statusLabel(item.previous_full_text_status)} full text
							</div>
							<div>
								<span class="font-medium">After:</span>
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
					</li>
				{/each}
			</ol>
		{/if}
	</Card.Content>
</Card.Root>
