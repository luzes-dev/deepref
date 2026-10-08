<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import {
		createGetAiReviewerAgreement,
		createListAiReviewerDecisions,
		createResolveAiReviewerConflict,
		getGetAiActivityOverviewQueryKey
	} from '$lib/api/generated/ai/ai';
	import { createListFullTextExclusionReasons } from '$lib/api/generated/documents/documents';
	import type { AiReviewerDecisionDto } from '$lib/api/generated/models';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as Select from '@deepref/ui/select';
	import * as ToggleGroup from '@deepref/ui/toggle-group';
	import { StatePanel } from '@deepref/ui/layout';
	import { resolve } from '$app/paths';
	import { notifyError, notifySuccess } from '$lib/features/notifications/toast';
	import { screeningKeys } from '$lib/features/screening/api';
	import { fullTextUrlString } from '$lib/features/full-text/url';
	import {
		CONFLICT_STATUS_LABEL,
		agreementSummary,
		decisionLabel,
		emptyConflictCopy,
		rationaleLines,
		type ConflictStatus
	} from '../conflicts';

	let { projectId, stage }: { projectId: string; stage: 'title_abstract' | 'full_text' } =
		$props();

	const queryClient = useQueryClient();
	let status = $state<ConflictStatus>('conflict');
	const listQuery = createListAiReviewerDecisions(
		() => projectId,
		() => ({ stage, status })
	);
	const agreementQuery = createGetAiReviewerAgreement(() => projectId);
	const reasonsQuery = createListFullTextExclusionReasons(() => projectId);
	const resolveMutation = createResolveAiReviewerConflict();

	let notes = $state<Record<string, string>>({});
	let reasonByDecision = $state<Record<string, string>>({});
	let busyId = $state<string | null>(null);
	let reasonMissing = $state<string | null>(null);

	const items = $derived(listQuery.data?.data ?? []);
	const agreement = $derived(agreementQuery.data?.data.find((item) => item.stage === stage));
	const reasons = $derived(reasonsQuery.data?.data ?? []);
	const statusOptions: ConflictStatus[] = ['conflict', 'waiting', 'concordant', 'resolved'];

	async function settle(
		item: AiReviewerDecisionDto,
		decision: 'include' | 'exclude' | 'maybe'
	): Promise<void> {
		if (busyId) return;
		const reason = reasonByDecision[item.id];
		if (decision === 'exclude' && stage === 'full_text' && !reason) {
			reasonMissing = item.id;
			return;
		}
		reasonMissing = null;
		busyId = item.id;
		try {
			await resolveMutation.mutateAsync({
				projectId,
				decisionId: item.id,
				data: {
					decision,
					exclusion_reason_id:
						decision === 'exclude' && stage === 'full_text' ? reason : undefined,
					note: notes[item.id]?.trim() || undefined
				}
			});
			notifySuccess('Conflict settled', `Final decision: ${decisionLabel(decision)}.`);
			await Promise.all([
				listQuery.refetch(),
				agreementQuery.refetch(),
				queryClient.invalidateQueries({ queryKey: screeningKeys.queue(projectId) }),
				queryClient.invalidateQueries({
					queryKey: getGetAiActivityOverviewQueryKey(projectId)
				})
			]);
		} catch (error) {
			notifyError('Could not settle this conflict', error, 'The decision was not saved.');
			await listQuery.refetch();
		} finally {
			busyId = null;
		}
	}

	/** Where a person screens this record. The AI's opinion is shown there only after they decide. */
	function screenHref(item: AiReviewerDecisionDto): string {
		return stage === 'full_text'
			? `${resolve('/projects/[projectId]/screening/full-text', { projectId })}${fullTextUrlString({ filter: 'all', report: item.report_id, page: null, block: null })}`
			: `${resolve('/projects/[projectId]/screening/title-abstract', { projectId })}?report=${encodeURIComponent(item.report_id)}`;
	}

	function badgeVariant(decision: string | null | undefined) {
		return decision === 'include'
			? 'success'
			: decision === 'exclude'
				? 'destructive'
				: 'warning';
	}
</script>

<section class="flex flex-col gap-4" data-testid="conflicts-panel" aria-label="Conflicts">
	<header class="flex flex-col gap-1">
		<h2 class="editorial-title text-xl">Conflicts with the AI reviewer</h2>
		<p class="max-w-2xl text-sm text-muted-foreground">
			The AI screened these records on its own. Where you decided differently, read both
			reasons and settle it. The AI never decides alone.
		</p>
		{#if agreement}
			<p class="text-xs text-muted-foreground" data-testid="conflicts-agreement">
				{agreementSummary(agreement)}
			</p>
		{/if}
	</header>

	<ToggleGroup.Root
		type="single"
		variant="outline"
		size="sm"
		class="w-fit"
		value={status}
		onValueChange={(value) => {
			if (value) status = value as ConflictStatus;
		}}
		aria-label="Show"
	>
		{#each statusOptions as option (option)}
			<ToggleGroup.Item value={option}>{CONFLICT_STATUS_LABEL[option]}</ToggleGroup.Item>
		{/each}
	</ToggleGroup.Root>

	{#if listQuery.isPending}
		<div class="flex flex-col gap-3">
			{#each { length: 3 }, index (index)}<Skeleton class="h-28 w-full" />{/each}
		</div>
	{:else if listQuery.error}
		<StatePanel
			state="error"
			title="Conflicts unavailable"
			description={listQuery.error.message}
		/>
	{:else if items.length === 0}
		<StatePanel
			state="empty"
			title={emptyConflictCopy(status, agreement?.compared ?? 0).title}
			description={emptyConflictCopy(status, agreement?.compared ?? 0).description}
		/>
	{:else}
		<ol class="flex flex-col" data-testid="conflicts-list">
			{#each items as item (item.id)}
				<li class="flex flex-col gap-3 border-b py-4" data-testid="conflict-item">
					<h3 class="text-sm font-semibold">{item.title ?? 'Untitled record'}</h3>
					<div class="grid gap-4 sm:grid-cols-2">
						<div class="flex flex-col gap-1.5">
							<p class="text-xs font-medium text-muted-foreground">You</p>
							<Badge
								variant={item.human_decision
									? badgeVariant(item.human_decision)
									: 'outline'}>{decisionLabel(item.human_decision)}</Badge
							>
							{#if item.human_notes}<p class="text-sm">{item.human_notes}</p>{/if}
						</div>
						<div class="flex flex-col gap-1.5">
							<p class="text-xs font-medium text-muted-foreground">
								AI reviewer{item.ai_model ? ` (${item.ai_model})` : ''}
							</p>
							{#if item.status === 'waiting'}
								<p
									class="text-sm text-muted-foreground"
									data-testid="conflict-ai-hidden"
								>
									The AI has screened this record. Its decision stays hidden until
									you decide, so your judgement is independent.
								</p>
							{:else}
								<Badge variant={badgeVariant(item.ai_decision)}
									>{decisionLabel(item.ai_decision)}</Badge
								>
								{#if rationaleLines(item.ai_rationale).length > 1}
									<ul class="flex list-disc flex-col gap-1 pl-4 text-sm">
										{#each rationaleLines(item.ai_rationale) as line, index (index)}
											<li>{line}</li>
										{/each}
									</ul>
								{:else}
									<p class="text-sm">{item.ai_rationale}</p>
								{/if}
								{#each item.ai_evidence as quote (quote.label + quote.quote)}
									<blockquote
										class="border-l-2 pl-3 text-xs text-muted-foreground"
									>
										<span class="font-medium text-foreground"
											>{quote.label}</span
										>
										<p class="mt-0.5">“{quote.quote}”</p>
									</blockquote>
								{/each}
							{/if}
						</div>
					</div>
					{#if item.status === 'conflict'}
						<div
							class="flex flex-col gap-2"
							role="group"
							aria-label="Settle this conflict"
						>
							<Input
								aria-label="Why you decided this way (optional)"
								placeholder="Why this decision? (optional)"
								value={notes[item.id] ?? ''}
								oninput={(event) =>
									(notes = { ...notes, [item.id]: event.currentTarget.value })}
							/>
							{#if stage === 'full_text'}
								<Select.Root
									type="single"
									value={reasonByDecision[item.id] ?? ''}
									onValueChange={(value) =>
										(reasonByDecision = {
											...reasonByDecision,
											[item.id]: value
										})}
								>
									<Select.Trigger aria-label="Reason for excluding" class="w-72"
										>{reasons.find(
											(reason) => reason.id === reasonByDecision[item.id]
										)?.label ?? 'Reason, if you exclude'}</Select.Trigger
									>
									<Select.Content>
										<Select.Group>
											{#each reasons as reason (reason.id)}
												<Select.Item
													value={reason.id}
													label={reason.label}
												/>
											{/each}
										</Select.Group>
									</Select.Content>
								</Select.Root>
								{#if reasonMissing === item.id}<p
										class="text-xs text-destructive"
										role="alert"
									>
										Choose a reason to exclude at full text.
									</p>{/if}
							{/if}
							<div class="flex flex-wrap gap-2">
								<span class="self-center text-xs text-muted-foreground"
									>Final decision:</span
								>
								{#each ['include', 'exclude', 'maybe'] as const as decision (decision)}
									<Button
										size="sm"
										variant="outline"
										disabled={busyId !== null}
										onclick={() => void settle(item, decision)}
										>{decisionLabel(decision)}</Button
									>
								{/each}
							</div>
						</div>
					{:else if item.status === 'resolved'}
						<p class="text-xs text-muted-foreground">
							Settled{item.resolution === 'adopted_ai'
								? ': you went with the AI.'
								: item.resolution === 'kept_human'
									? ': you kept your decision.'
									: '.'}
						</p>
					{:else if item.status === 'waiting'}
						<div class="flex flex-wrap items-center gap-3">
							<p class="text-xs text-muted-foreground">
								Decide this record first. The AI's view and any conflict appear here
								afterwards.
							</p>
							<Button size="sm" variant="outline" href={screenHref(item)}>
								Screen this record
							</Button>
						</div>
					{/if}
				</li>
			{/each}
		</ol>
	{/if}
</section>
