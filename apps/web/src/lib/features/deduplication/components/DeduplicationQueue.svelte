<script lang="ts">
	import {
		createDecideProjectDedupeProposal,
		createListProjectDedupeProposals,
		createRunProjectDeduplication,
		getListProjectDedupeProposalsQueryKey
	} from '$lib/api/generated/deduplication/deduplication';
	import { getGetProjectPrismaQueryKey } from '$lib/api/generated/review/review';
	import { getListProjectReportsQueryKey } from '$lib/api/generated/reports/reports';
	import type {
		DedupeProposalDto,
		ProposalDecisionInput,
		RunDeduplicationRequest
	} from '$lib/api/generated/models';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Spinner } from '$lib/components/ui/spinner';
	import { PageHeader, PageToolbar, StatePanel, Surface } from '$lib/components/layout';
	import {
		displayDedupeTitle,
		formatDedupeJson,
		formatDedupeScore,
		formatDedupeYear
	} from '$lib/features/deduplication/formatters';
	import { useQueryClient } from '@tanstack/svelte-query';
	import AiProposalReview from '$lib/features/ai-assistance/components/AiProposalReview.svelte';
	import CheckIcon from '@lucide/svelte/icons/check';
	import GitCompareIcon from '@lucide/svelte/icons/git-compare';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import XIcon from '@lucide/svelte/icons/x';

	let { projectId }: { projectId: string } = $props();

	const queryClient = useQueryClient();
	const pendingParams = { limit: 100, status: 'pending' };
	const proposalsQuery = createListProjectDedupeProposals(
		() => projectId,
		() => pendingParams
	);
	const decideProposal = createDecideProjectDedupeProposal();
	const runDeduplication = createRunProjectDeduplication();

	let pendingProposalId = $state<string | null>(null);
	let isRunning = $state(false);
	let runSummary = $state<string | null>(null);

	const proposals = $derived(proposalsQuery.data?.data.items ?? []);
	const errorMessage = $derived(
		decideProposal.error?.message ??
			runDeduplication.error?.message ??
			proposalsQuery.error?.message
	);

	async function refreshProposalList() {
		await Promise.all([
			queryClient.invalidateQueries({
				queryKey: getListProjectDedupeProposalsQueryKey(projectId, pendingParams)
			}),
			queryClient.invalidateQueries({ queryKey: getListProjectReportsQueryKey(projectId) }),
			queryClient.invalidateQueries({ queryKey: getGetProjectPrismaQueryKey(projectId) })
		]);
		await proposalsQuery.refetch();
	}

	async function decide(proposal: DedupeProposalDto, decision: ProposalDecisionInput) {
		if (pendingProposalId || runDeduplication.isPending) return;
		pendingProposalId = proposal.id;
		try {
			await decideProposal.mutateAsync({
				projectId,
				proposalId: proposal.id,
				data: {
					decision,
					reason: `Manual deduplication decision: ${decision}`,
					actor_kind: 'user'
				}
			});
			await refreshProposalList();
		} catch {
			// The mutation error is rendered above the queue.
		} finally {
			pendingProposalId = null;
		}
	}

	async function run() {
		if (isRunning || pendingProposalId) return;
		isRunning = true;
		runSummary = null;
		const request: RunDeduplicationRequest = { limit: 100, actor_kind: 'user' };
		try {
			const response = await runDeduplication.mutateAsync({ projectId, data: request });
			const result = response.data;
			runSummary = `Processed ${result.processed}: ${result.auto_linked} linked, ${result.created_reports} new reports, ${result.proposals_created} proposals, ${result.conflicts} conflicts.`;
			await refreshProposalList();
		} catch {
			// The mutation error is rendered above the queue.
		} finally {
			isRunning = false;
		}
	}
</script>

<div
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	data-testid="deduplication-page"
>
	<div class="mx-auto flex w-full max-w-[1440px] flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Evidence workspace / Review"
			title="Resolve duplicate records"
			description="Review deterministic identifier conflicts and explainable fuzzy candidates. Source records remain available for PRISMA accounting."
		>
			{#snippet actions()}
				<Button onclick={run} disabled={isRunning || pendingProposalId !== null}>
					{#if isRunning}<Spinner data-icon="inline-start" />{:else}<RefreshCwIcon
							data-icon="inline-start"
						/>{/if}
					Run deduplication
				</Button>
			{/snippet}
		</PageHeader>

		<PageToolbar label="Deduplication queue status">
			<div class="flex flex-wrap items-center gap-2">
				<GitCompareIcon aria-hidden="true" />
				<Badge variant="secondary">{proposals.length} pending</Badge>
				<Badge variant={isRunning ? 'outline' : 'default'}>
					{isRunning ? 'Processing' : 'Ready for review'}
				</Badge>
			</div>
		</PageToolbar>

		{#if errorMessage}
			<Alert.Root variant="destructive" data-testid="deduplication-error">
				<Alert.Title>Deduplication could not continue</Alert.Title>
				<Alert.Description>{errorMessage}</Alert.Description>
			</Alert.Root>
		{/if}

		{#if runSummary}
			<Alert.Root data-testid="deduplication-run-summary">
				<Alert.Title>Deduplication run complete</Alert.Title>
				<Alert.Description>{runSummary}</Alert.Description>
			</Alert.Root>
		{/if}

		<Surface
			as="section"
			tone="default"
			class="flex flex-col gap-5 p-4 sm:p-5"
			label="Pending proposals"
		>
			<div class="flex flex-wrap items-center justify-between gap-2">
				<div class="flex flex-col gap-1">
					<h2 id="pending-proposals-heading" class="text-xl font-semibold">
						Pending proposals
					</h2>
					<p class="text-sm text-muted-foreground">
						Fuzzy matches are proposals only; exact non-conflicting identifiers are
						resolved automatically.
					</p>
				</div>
				<Badge variant="secondary">{proposals.length} pending</Badge>
			</div>

			{#if proposalsQuery.isPending}
				<div aria-label="Loading deduplication proposals">
					<StatePanel
						state="loading"
						title="Loading deduplication proposals"
						description="Checking source-record identity signals."
					/>
				</div>
			{:else if proposals.length === 0}
				<div data-testid="deduplication-empty">
					<StatePanel
						state="empty"
						title="No pending proposals"
						description="Run a bounded pass after importing records, or return here when a reviewer needs to resolve a candidate."
					/>
				</div>
			{:else}
				<div class="grid gap-4 lg:grid-cols-2">
					{#each proposals as proposal (proposal.id)}
						<Card.Root data-testid="deduplication-proposal">
							<Card.Header class="gap-3">
								<div class="flex flex-wrap items-center justify-between gap-2">
									<Badge
										variant={proposal.conflicting_identifier
											? 'destructive'
											: 'outline'}
									>
										{proposal.proposal_kind === 'conflict'
											? 'Identifier conflict'
											: 'Fuzzy candidate'}
									</Badge>
									<span class="text-sm text-muted-foreground"
										>Score {formatDedupeScore(proposal.score)}</span
									>
								</div>
								<Card.Title>{displayDedupeTitle(proposal.source_title)}</Card.Title>
								<Card.Description
									>Source record {proposal.record_id}</Card.Description
								>
							</Card.Header>
							<Card.Content class="flex flex-col gap-4">
								<div class="grid gap-4 md:grid-cols-2">
									<section
										class="flex flex-col gap-2 rounded-lg border p-3"
										aria-label="Source record"
									>
										<h3 class="font-medium">Source record</h3>
										<p class="text-sm">
											{displayDedupeTitle(proposal.source_title)}
										</p>
										<p class="text-xs text-muted-foreground">
											Year: {formatDedupeYear(proposal.source_year)}
										</p>
										<p class="text-xs text-muted-foreground">
											Authors: {formatDedupeJson(proposal.source_authors)}
										</p>
										<p class="text-xs text-muted-foreground">
											Identifiers: {formatDedupeJson(
												proposal.source_identifiers
											)}
										</p>
									</section>
									<section
										class="flex flex-col gap-2 rounded-lg border p-3"
										aria-label="Candidate report"
									>
										<h3 class="font-medium">Candidate report</h3>
										<p class="text-sm">
											{displayDedupeTitle(proposal.candidate_title)}
										</p>
										<p class="text-xs text-muted-foreground">
											Year: {formatDedupeYear(proposal.candidate_year)}
										</p>
										<p class="text-xs text-muted-foreground">
											Authors: {formatDedupeJson(proposal.candidate_authors)}
										</p>
										<p class="text-xs text-muted-foreground">
											Identifiers: {formatDedupeJson(
												proposal.candidate_identifiers
											)}
										</p>
									</section>
								</div>

								<dl
									class="grid gap-2 rounded-lg bg-muted/50 p-3 text-sm sm:grid-cols-3"
								>
									<div>
										<dt class="text-muted-foreground">Title similarity</dt>
										<dd class="font-medium">
											{formatDedupeScore(proposal.title_similarity)}
										</dd>
									</div>
									<div>
										<dt class="text-muted-foreground">Year</dt>
										<dd class="font-medium">
											{proposal.year_match === null ||
											proposal.year_match === undefined
												? 'Not compared'
												: proposal.year_match
													? 'Match'
													: 'Different'}
										</dd>
									</div>
									<div>
										<dt class="text-muted-foreground">First author</dt>
										<dd class="font-medium">
											{formatDedupeScore(proposal.first_author_similarity)}
										</dd>
									</div>
								</dl>
							</Card.Content>
							<Card.Footer class="flex flex-wrap justify-end gap-2">
								<Button
									variant="outline"
									disabled={pendingProposalId !== null}
									onclick={() => void decide(proposal, 'reject')}
								>
									{#if pendingProposalId === proposal.id}<Spinner
											data-icon="inline-start"
										/>{:else}<XIcon data-icon="inline-start" />{/if}
									Reject
								</Button>
								{#if proposal.proposal_kind !== 'conflict'}
									<Button
										variant="secondary"
										disabled={pendingProposalId !== null}
										onclick={() => void decide(proposal, 'create_new')}
									>
										<PlusIcon data-icon="inline-start" />Create new report
									</Button>
								{/if}
								<Button
									disabled={pendingProposalId !== null}
									onclick={() => void decide(proposal, 'accept')}
								>
									{#if pendingProposalId === proposal.id}<Spinner
											data-icon="inline-start"
										/>{:else}<CheckIcon data-icon="inline-start" />{/if}
									Accept candidate
								</Button>
							</Card.Footer>
							<AiProposalReview
								{projectId}
								stage="dedupe"
								recordId={proposal.record_id}
								candidateReportId={proposal.candidate_report_id}
							/>
						</Card.Root>
					{/each}
				</div>
			{/if}
		</Surface>
	</div>
</div>
