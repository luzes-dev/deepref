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
	import * as Alert from '@deepref/ui/alert';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import * as Card from '@deepref/ui/card';
	import { Spinner } from '@deepref/ui/spinner';
	import { PageHeader, PageToolbar, StatePanel, Surface } from '@deepref/ui/layout';
	import {
		displayDedupeTitle,
		formatDedupeJson,
		formatDedupeScore,
		formatDedupeYear
	} from '$lib/features/deduplication/formatters';
	import { useQueryClient } from '@tanstack/svelte-query';
	import AiProposalReview from '$lib/features/ai-assistance/components/AiProposalReview.svelte';
	import { notifyError, notifySuccess } from '$lib/features/notifications/toast';
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

	const proposals = $derived(proposalsQuery.data?.data.items ?? []);
	const errorMessage = $derived(proposalsQuery.error?.message);

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
		} catch (error) {
			notifyError('The decision could not be saved', error);
		} finally {
			pendingProposalId = null;
		}
	}

	async function run() {
		if (isRunning || pendingProposalId) return;
		isRunning = true;
		const request: RunDeduplicationRequest = { limit: 100, actor_kind: 'user' };
		try {
			const response = await runDeduplication.mutateAsync({ projectId, data: request });
			const result = response.data;
			notifySuccess(
				'Deduplication run complete',
				`Processed ${result.processed}: ${result.auto_linked} linked, ${result.created_reports} new reports, ${result.proposals_created} proposals, ${result.conflicts} conflicts.`
			);
			await refreshProposalList();
		} catch (error) {
			notifyError('Deduplication could not continue', error);
		} finally {
			isRunning = false;
		}
	}
</script>

<div
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	data-testid="deduplication-page"
>
	<div class="mx-auto flex w-full max-w-[1536px] flex-col gap-6 p-4 sm:p-6 lg:p-8">
		<PageHeader
			title="Resolve duplicate records"
			description="Compare possible duplicates and decide which records belong to the same article."
		>
			{#snippet actions()}
				<Button onclick={run} disabled={isRunning || pendingProposalId !== null}>
					{#if isRunning}
						<Spinner data-icon="inline-start" />
					{:else}
						<RefreshCwIcon data-icon="inline-start" />
					{/if}
					Run deduplication
				</Button>
			{/snippet}
		</PageHeader>

		<PageToolbar label="Deduplication queue status">
			<div class="flex flex-wrap items-center gap-2">
				<GitCompareIcon class="size-4 text-muted-foreground" aria-hidden="true" />
				<Badge variant="secondary">{proposals.length} pending</Badge>
				<Badge variant={isRunning ? 'outline' : 'default'}>
					{isRunning ? 'Processing' : 'Ready for review'}
				</Badge>
			</div>
		</PageToolbar>

		{#if errorMessage}
			<Alert.Root variant="destructive" data-testid="deduplication-error">
				<Alert.Title>Deduplication queue unavailable</Alert.Title>
				<Alert.Description>{errorMessage}</Alert.Description>
			</Alert.Root>
		{/if}

		<Surface
			as="section"
			tone="default"
			class="flex flex-col gap-5 p-4 sm:p-6"
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
						description="Run deduplication to check your imported articles for possible matches."
					/>
				</div>
			{:else}
				<!-- Cards Pattern Grid for Proposals -->
				<div class="flex flex-col gap-6">
					{#each proposals as proposal (proposal.id)}
						<Card.Root
							data-testid="deduplication-proposal"
							class="overflow-hidden border-2 shadow-sm transition-shadow hover:shadow-md"
						>
							<!-- Card Header with badges and similarity score -->
							<Card.Header class="border-b bg-muted/20 pb-4">
								<div class="flex flex-wrap items-center justify-between gap-3">
									<div class="flex items-center gap-2">
										<Badge
											variant={proposal.conflicting_identifier
												? 'destructive'
												: 'outline'}
										>
											{proposal.proposal_kind === 'conflict'
												? 'Identifier conflict'
												: 'Fuzzy candidate'}
										</Badge>
										{#if proposal.score >= 0.9}
											<Badge variant="success">High confidence</Badge>
										{:else if proposal.score >= 0.75}
											<Badge variant="secondary">Medium confidence</Badge>
										{/if}
									</div>
									<div
										class="flex items-center gap-2 font-mono text-sm font-semibold"
									>
										<span class="text-xs font-normal text-muted-foreground"
											>Match Score:</span
										>
										<span class="text-sm text-muted-foreground"
											>Score {formatDedupeScore(proposal.score)}</span
										>
									</div>
								</div>
								<div class="mt-2 space-y-1">
									<Card.Title class="text-base font-semibold">
										{displayDedupeTitle(proposal.source_title)}
									</Card.Title>
									<Card.Description class="text-xs">
										Source record {proposal.record_id}
									</Card.Description>
								</div>
							</Card.Header>

							<Card.Content class="flex flex-col gap-5 pt-5">
								<!-- Side-by-Side Comparison Columns (Cards Pattern) -->
								<div class="grid gap-4 md:grid-cols-2">
									<!-- Source Record Card Column -->
									<section
										class="flex flex-col justify-between rounded-lg border bg-card/50 p-4 shadow-sm"
										aria-label="Source record"
									>
										<div class="space-y-2.5">
											<div
												class="flex items-center justify-between border-b pb-2"
											>
												<h3
													class="text-xs font-semibold tracking-wider text-muted-foreground uppercase"
												>
													Source record
												</h3>
												<Badge variant="outline" class="text-[10px]"
													>Imported</Badge
												>
											</div>
											<p class="text-sm leading-snug font-medium">
												{displayDedupeTitle(proposal.source_title)}
											</p>
										</div>

										<div
											class="mt-4 space-y-1.5 border-t pt-3 text-xs text-muted-foreground"
										>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Year:</span
												>
												<span>{formatDedupeYear(proposal.source_year)}</span
												>
											</div>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Authors:</span
												>
												<span class="max-w-[200px] truncate text-right">
													{formatDedupeJson(proposal.source_authors)}
												</span>
											</div>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Identifiers:</span
												>
												<span
													class="max-w-[200px] truncate text-right font-mono text-[11px]"
												>
													{formatDedupeJson(proposal.source_identifiers)}
												</span>
											</div>
										</div>
									</section>

									<!-- Candidate Report Card Column -->
									<section
										class="flex flex-col justify-between rounded-lg border border-primary/20 bg-primary/[0.02] p-4 shadow-sm"
										aria-label="Candidate report"
									>
										<div class="space-y-2.5">
											<div
												class="flex items-center justify-between border-b pb-2"
											>
												<h3
													class="text-xs font-semibold tracking-wider text-primary uppercase"
												>
													Candidate report
												</h3>
												<Badge variant="secondary" class="text-[10px]"
													>Existing Library</Badge
												>
											</div>
											<p class="text-sm leading-snug font-medium">
												{displayDedupeTitle(proposal.candidate_title)}
											</p>
										</div>

										<div
											class="mt-4 space-y-1.5 border-t pt-3 text-xs text-muted-foreground"
										>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Year:</span
												>
												<span
													>{formatDedupeYear(
														proposal.candidate_year
													)}</span
												>
											</div>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Authors:</span
												>
												<span class="max-w-[200px] truncate text-right">
													{formatDedupeJson(proposal.candidate_authors)}
												</span>
											</div>
											<div class="flex justify-between">
												<span class="font-medium text-foreground"
													>Identifiers:</span
												>
												<span
													class="max-w-[200px] truncate text-right font-mono text-[11px]"
												>
													{formatDedupeJson(
														proposal.candidate_identifiers
													)}
												</span>
											</div>
										</div>
									</section>
								</div>

								<!-- Signal Comparison Summary Strip -->
								<dl
									class="grid gap-3 rounded-lg border bg-muted/30 p-3.5 text-xs sm:grid-cols-3"
								>
									<div class="space-y-0.5">
										<dt class="text-muted-foreground">Title similarity</dt>
										<dd class="text-sm font-semibold">
											{formatDedupeScore(proposal.title_similarity)}
										</dd>
									</div>
									<div class="space-y-0.5">
										<dt class="text-muted-foreground">Year</dt>
										<dd class="text-sm font-semibold">
											{proposal.year_match === null ||
											proposal.year_match === undefined
												? 'Not compared'
												: proposal.year_match
													? 'Match'
													: 'Different'}
										</dd>
									</div>
									<div class="space-y-0.5">
										<dt class="text-muted-foreground">First author</dt>
										<dd class="text-sm font-semibold">
											{formatDedupeScore(proposal.first_author_similarity)}
										</dd>
									</div>
								</dl>
							</Card.Content>

							<!-- Action Footer -->
							<Card.Footer
								class="flex flex-wrap items-center justify-end gap-2 border-t bg-muted/10 py-3"
							>
								<Button
									variant="outline"
									size="sm"
									disabled={pendingProposalId !== null}
									onclick={() => void decide(proposal, 'reject')}
								>
									{#if pendingProposalId === proposal.id}
										<Spinner data-icon="inline-start" />
									{:else}
										<XIcon data-icon="inline-start" />
									{/if}
									Reject
								</Button>
								{#if proposal.proposal_kind !== 'conflict'}
									<Button
										variant="secondary"
										size="sm"
										disabled={pendingProposalId !== null}
										onclick={() => void decide(proposal, 'create_new')}
									>
										<PlusIcon data-icon="inline-start" />
										Create new report
									</Button>
								{/if}
								<Button
									size="sm"
									disabled={pendingProposalId !== null}
									onclick={() => void decide(proposal, 'accept')}
								>
									{#if pendingProposalId === proposal.id}
										<Spinner data-icon="inline-start" />
									{:else}
										<CheckIcon data-icon="inline-start" />
									{/if}
									Accept candidate
								</Button>
							</Card.Footer>

							<!-- Grounded AI Assistant Proposal Module -->
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
