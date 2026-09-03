<script lang="ts">
	import type { StudyEventDto } from '$lib/api/generated/models';
	import { Surface, StatePanel } from '$lib/components/layout';
	import { Badge } from '$lib/components/ui/badge';

	let { history }: { history: StudyEventDto[] } = $props();
</script>

<Surface as="section" tone="inset" class="flex flex-col gap-4 p-4 sm:p-5" label="Grouping history">
	<div class="border-b border-border/70 pb-4">
		<h2 class="text-lg font-semibold">Grouping history</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			Every group, move, unassign, rename, and classification change remains visible.
		</p>
	</div>
	{#if history.length === 0}
		<StatePanel
			state="empty"
			title="No history yet."
			description="Audited study changes will appear here as the investigation evolves."
		/>
	{:else}
		<ol class="flex flex-col gap-3" aria-live="polite">
			{#each history as event (event.id)}
				<li class="rounded-md border border-border/70 bg-background/50 p-3 text-sm">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<Badge variant="outline">{event.event_type}</Badge>
						<span class="text-xs text-muted-foreground">
							{new Date(event.created_at).toLocaleString()} · {event.actor_id}
						</span>
					</div>
					<p class="mt-2 text-xs text-muted-foreground">
						revision {event.before_revision} → {event.result_revision}{event.report_id
							? ` · report ${event.report_id}`
							: ''}
					</p>
				</li>
			{/each}
		</ol>
	{/if}
</Surface>
