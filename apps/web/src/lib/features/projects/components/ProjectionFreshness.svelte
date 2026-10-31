<script lang="ts">
	import { Badge, type BadgeVariant } from '@deepref/ui/badge';
	import type { ProjectionMetadata } from '#lib/api/generated/models/index.js';

	let { projection }: { projection: ProjectionMetadata } = $props();
	const states = {
		pending: {
			label: 'Metrics pending',
			detail: 'Metrics have not been calculated yet.',
			variant: 'outline'
		},
		ready: { label: 'Metrics current', detail: '', variant: 'secondary' },
		lagging: {
			label: 'Metrics out of date',
			detail: 'Recent changes are not reflected in these values yet.',
			variant: 'warning'
		},
		rebuilding: {
			label: 'Metrics rebuilding',
			detail: 'Existing values may be shown while metrics are rebuilt.',
			variant: 'warning'
		},
		failed: {
			label: 'Metrics refresh failed',
			detail: 'Values from the last successful calculation may still be shown.',
			variant: 'destructive'
		}
	} satisfies Record<
		ProjectionMetadata['state'],
		{ label: string; detail: string; variant: BadgeVariant }
	>;
	const presentation = $derived(states[projection.state]);
</script>

<div role="status" aria-label="Metric freshness" class="flex flex-wrap items-center gap-2">
	<Badge variant={presentation.variant}>{presentation.label}</Badge>
	{#if presentation.detail}<p class="text-xs text-muted-foreground">{presentation.detail}</p>{/if}
</div>
