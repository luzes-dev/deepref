<script lang="ts">
	import type { EligibilityCriterionDto } from '$lib/api/generated/models';

	let {
		criteria,
		protocolVersion,
		stage = 'title_abstract'
	}: {
		criteria: EligibilityCriterionDto[];
		protocolVersion: number | undefined;
		stage?: 'title_abstract' | 'full_text';
	} = $props();

	const stageCriteria = $derived(
		criteria.filter((criterion) => criterion.stage === stage || criterion.stage === 'both')
	);
	const groups = $derived([
		['Include when', stageCriteria.filter((criterion) => criterion.kind !== 'exclusion')],
		['Exclude when', stageCriteria.filter((criterion) => criterion.kind === 'exclusion')]
	] as const);
</script>

<section class="flex flex-col gap-4" aria-labelledby="criteria-title-{stage}">
	<div class="flex items-baseline justify-between gap-3">
		<h3 id="criteria-title-{stage}" class="text-sm font-semibold">Eligibility criteria</h3>
		<span class="text-xs text-muted-foreground tabular-nums"
			>Protocol v{protocolVersion ?? '—'}</span
		>
	</div>
	{#if stageCriteria.length > 0}
		{#each groups as [heading, items] (heading)}
			{#if items.length > 0}
				<div class="flex flex-col gap-3">
					<p class="text-2xs font-semibold tracking-caps text-muted-foreground uppercase">
						{heading}
					</p>
					<ol class="flex flex-col gap-3">
						{#each items as criterion (criterion.id)}
							<li class="flex flex-col gap-0.5 text-sm">
								<span class="font-medium">{criterion.label}</span>
								<span class="leading-6 text-muted-foreground"
									>{criterion.description}</span
								>
							</li>
						{/each}
					</ol>
				</div>
			{/if}
		{/each}
	{:else}
		<p class="text-sm text-muted-foreground">
			No {stage === 'full_text' ? 'full-text' : 'title/abstract'} criteria are published.
		</p>
	{/if}
</section>
