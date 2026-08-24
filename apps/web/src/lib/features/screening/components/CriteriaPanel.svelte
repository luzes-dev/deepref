<script lang="ts">
	import type { EligibilityCriterionDto } from '$lib/api/generated/models';
	import * as Card from '$lib/components/ui/card';

	let {
		criteria,
		protocolVersion
	}: {
		criteria: EligibilityCriterionDto[];
		protocolVersion: number | undefined;
	} = $props();

	const titleAbstractCriteria = $derived(
		criteria.filter(
			(criterion) => criterion.stage === 'title_abstract' || criterion.stage === 'both'
		)
	);
</script>

<Card.Root>
	<Card.Header>
		<Card.Title>Eligibility criteria</Card.Title>
		<Card.Description
			>Title/abstract criteria · Protocol v{protocolVersion ?? '—'}</Card.Description
		>
	</Card.Header>
	<Card.Content>
		{#if titleAbstractCriteria.length > 0}
			<ol class="flex flex-col gap-4">
				{#each titleAbstractCriteria as criterion (criterion.id)}
					<li class="flex flex-col gap-1">
						<div class="flex flex-wrap items-center gap-2">
							<span class="text-sm font-medium">{criterion.label}</span>
							<span
								class="rounded-full border px-2 py-0.5 text-[10px] tracking-wide text-muted-foreground uppercase"
								>{criterion.kind}</span
							>
							<span
								class="rounded-full border px-2 py-0.5 text-[10px] tracking-wide text-muted-foreground uppercase"
								>{criterion.dimension}</span
							>
						</div>
						<span class="text-xs text-muted-foreground"
							>Stage: {criterion.stage.replace('_', ' ')}</span
						>
						<span class="text-sm text-muted-foreground">{criterion.description}</span>
					</li>
				{/each}
			</ol>
		{:else}
			<p class="text-sm text-muted-foreground">No title/abstract criteria are published.</p>
		{/if}
	</Card.Content>
</Card.Root>
