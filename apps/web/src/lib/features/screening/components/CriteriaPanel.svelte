<script lang="ts">
	import type { EligibilityCriterionDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import * as Card from '$lib/components/ui/card';
	import { ClipboardCheck, Info } from '@lucide/svelte';

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
</script>

<Card.Root class="border-primary/15">
	<Card.Header class="gap-2 border-b border-border/60 pb-4">
		<div class="flex items-start justify-between gap-3">
			<div class="flex items-center gap-2">
				<span
					class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
				>
					<ClipboardCheck aria-hidden="true" />
				</span>
				<Card.Title>Eligibility criteria</Card.Title>
			</div>
			<Badge variant="outline">v{protocolVersion ?? '—'}</Badge>
		</div>
		<Card.Description
			>{stage === 'full_text' ? 'Full-text' : 'Title/abstract'} criteria · Apply the published protocol
			consistently.</Card.Description
		>
	</Card.Header>
	<Card.Content class="pt-0">
		{#if stageCriteria.length > 0}
			<ol class="flex flex-col divide-y divide-border/60">
				{#each stageCriteria as criterion (criterion.id)}
					<li class="flex gap-3 py-4 first:pt-1 last:pb-1">
						<span
							class="flex size-6 shrink-0 items-center justify-center rounded-full border border-primary/30 text-xs font-semibold text-primary"
							>{criterion.ordinal}</span
						>
						<div class="flex min-w-0 flex-1 flex-col gap-2">
							<div class="flex flex-wrap items-center gap-2">
								<span class="text-sm font-semibold">{criterion.label}</span>
								<Badge
									variant={criterion.kind === 'exclusion'
										? 'destructive'
										: 'secondary'}>{criterion.kind}</Badge
								>
							</div>
							<p class="text-sm leading-6 text-muted-foreground">
								{criterion.description}
							</p>
							<span class="text-[11px] tracking-wide text-muted-foreground uppercase"
								>{criterion.dimension} · {criterion.stage.replace('_', ' ')}</span
							>
						</div>
					</li>
				{/each}
			</ol>
		{:else}
			<div
				class="flex items-start gap-2 rounded-lg border border-dashed p-3 text-sm text-muted-foreground"
			>
				<Info aria-hidden="true" />
				<p>No title/abstract criteria are published.</p>
			</div>
		{/if}
	</Card.Content>
</Card.Root>
