<script lang="ts">
	import type { StudyEventDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';

	let { history }: { history: StudyEventDto[] } = $props();
</script>

<Card.Root>
	<Card.Header>
		<Card.Title>Grouping history</Card.Title>
		<Card.Description>
			Every group, move, unassign, rename, and classification change remains visible.
		</Card.Description>
	</Card.Header>
	<Card.Content>
		<ol class="flex flex-col gap-3" aria-live="polite">
			{#each history as event (event.id)}
				<li class="rounded-md border p-3 text-sm">
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
			{:else}
				<li class="text-sm text-muted-foreground">No history yet.</li>
			{/each}
		</ol>
	</Card.Content>
</Card.Root>
