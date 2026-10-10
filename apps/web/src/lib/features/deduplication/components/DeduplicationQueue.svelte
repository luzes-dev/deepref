<script lang="ts">
	import {
		createDecideProjectDedupeProposal,
		createListProjectDedupeProposals,
		createRunProjectDeduplication,
		getListProjectDedupeProposalsQueryKey
	} from '#lib/api/generated/deduplication/deduplication.js';
	import { getGetProjectPrismaQueryKey } from '#lib/api/generated/review/review.js';
	import { getListProjectReportsQueryKey } from '#lib/api/generated/reports/reports.js';
	import type {
		DedupeProposalDto,
		DedupeRunDto,
		ProposalDecisionInput,
		RunDeduplicationRequest
	} from '#lib/api/generated/models/index.js';
	import * as Alert from '@deepref/ui/alert';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import * as Card from '@deepref/ui/card';
	import * as Tooltip from '@deepref/ui/tooltip';
	import { Spinner } from '@deepref/ui/spinner';
	import { StatePanel } from '@deepref/ui/layout';
	import {
		displayDedupeTitle,
		formatDedupeRunSummary,
		formatDedupeScore,
		formatDedupeYear
	} from '#lib/features/deduplication/formatters.js';
	import { useQueryClient } from '@tanstack/svelte-query';
	import AiProposalReview from '#lib/features/ai-assistance/components/AiProposalReview.svelte';
	import { notifyError, notifySuccess } from '#lib/features/notifications/toast.js';
	import CheckIcon from '@lucide/svelte/icons/check';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import XIcon from '@lucide/svelte/icons/x';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';

	function identifierLabels(identifiers: unknown): string[] {
		if (!Array.isArray(identifiers)) return [];
		return identifiers.flatMap((identifier: unknown) => {
			if (
				!identifier ||
				typeof identifier !== 'object' ||
				!('scheme' in identifier) ||
				!('value' in identifier)
			)
				return [];
			if (typeof identifier.scheme !== 'string' || typeof identifier.value !== 'string')
				return [];
			return [`${identifier.scheme.toUpperCase()}: ${identifier.value}`];
		});
	}
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
	let lastRun = $state<{ result: DedupeRunDto; at: number } | null>(null);
	let now = $state(Date.now());
	let selectedProposalId = $state<string | null>(null);

	const proposals = $derived(proposalsQuery.data?.data.items ?? []);
	const currentProposal = $derived(
		proposals.find((proposal) => proposal.id === selectedProposalId) ?? proposals[0]
	);
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

	const lastRunLabel = $derived.by(() => {
		if (!lastRun) return '';
		const seconds = Math.max(0, Math.round((now - lastRun.at) / 1000));
		const when = seconds < 60 ? 'just now' : `${Math.round(seconds / 60)} min ago`;
		return formatDedupeRunSummary(lastRun.result, when);
	});

	$effect(() => {
		if (!lastRun) return;
		const timer = setInterval(() => (now = Date.now()), 30_000);
		return () => clearInterval(timer);
	});

	async function run() {
		if (isRunning || pendingProposalId) return;
		isRunning = true;
		const request: RunDeduplicationRequest = { limit: 100, actor_kind: 'user' };
		try {
			const response = await runDeduplication.mutateAsync({ projectId, data: request });
			const result = response.data;
			lastRun = { result, at: Date.now() };
			now = Date.now();
			notifySuccess(
				'Deduplication run complete',
				`${formatDedupeRunSummary(result, 'just now')}. ${result.created_reports} new reports, ${result.conflicts} conflicts.`
			);
			await refreshProposalList();
		} catch (error) {
			notifyError('Deduplication could not continue', error);
		} finally {
			isRunning = false;
		}
	}
</script>

<PageTemplate testId="deduplication-page" maxWidth="wide">
	{#if errorMessage}
		<Alert.Root variant="destructive" data-testid="deduplication-error">
			<Alert.Title>Deduplication queue unavailable</Alert.Title>
			<Alert.Description>{errorMessage}</Alert.Description>
		</Alert.Root>
	{/if}

	<section class="flex flex-col gap-5" aria-labelledby="pending-proposals-heading">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div class="flex flex-col gap-1">
				<h2 id="pending-proposals-heading" class="editorial-title text-xl">
					Possible duplicates
					<span class="font-normal text-muted-foreground tabular-nums"
						>{proposals.length}</span
					>
				</h2>
				<p class="text-sm text-muted-foreground">
					Exact identifier matches merge automatically. Fuzzy matches wait here for your
					decision.
				</p>
			</div>
			<Button
				variant={proposals.length > 0 ? 'outline' : 'default'}
				onclick={run}
				disabled={isRunning || pendingProposalId !== null}
			>
				{#if isRunning}
					<Spinner data-icon="inline-start" />
				{:else}
					<RefreshCwIcon data-icon="inline-start" />
				{/if}
				{isRunning ? 'Checking…' : 'Run deduplication'}
			</Button>
		</div>

		{#if isRunning || lastRun}
			<p
				class="text-sm text-muted-foreground tabular-nums"
				role="status"
				data-testid="deduplication-run-result"
			>
				{isRunning ? 'Checking records for duplicates…' : lastRunLabel}
			</p>
		{/if}

		{#if proposalsQuery.isPending}
			<div aria-label="Loading deduplication proposals">
				<StatePanel
					state="loading"
					title="Loading deduplication proposals"
					description="Checking source-record identity signals."
				/>
			</div>
		{:else if proposals.length === 0}
			<div class="flex flex-col gap-1 border-t pt-5" data-testid="deduplication-empty">
				<p class="font-medium">No pending proposals</p>
				<p class="text-sm text-muted-foreground">
					Run deduplication after each import to look for records that describe the same
					article.
				</p>
			</div>
		{:else}
			<div class="grid gap-6 lg:grid-cols-4">
				<nav class="flex flex-col gap-1 lg:col-span-1" aria-label="Duplicate candidates">
					{#each proposals as proposal (proposal.id)}
						<Button
							variant={currentProposal?.id === proposal.id ? 'secondary' : 'ghost'}
							size="multiline"
							class="w-full"
							onclick={() => (selectedProposalId = proposal.id)}
							aria-current={currentProposal?.id === proposal.id ? 'true' : undefined}
						>
							{displayDedupeTitle(proposal.source_title)}
						</Button>
					{/each}
				</nav>
				<div class="min-w-0 lg:col-span-3">
					{#each currentProposal ? [currentProposal] : [] as proposal (proposal.id)}
						<Card.Root
							data-testid="deduplication-proposal"
							class="overflow-hidden rounded-none border-0 border-t shadow-none"
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
									<p class="flex items-baseline gap-2 text-sm">
										<span class="text-xs text-muted-foreground"
											>Match score</span
										>
										<span class="font-semibold tabular-nums"
											>{formatDedupeScore(proposal.score)}</span
										>
									</p>
								</div>
							</Card.Header>

							<Card.Content class="p-4 sm:p-6">
								<!-- 2-Column Split: Incoming Record vs Existing Candidate -->
								<div class="grid gap-6 md:grid-cols-2">
									<!-- Left Column: Source Record (Incoming) -->
									<section
										class="flex min-w-0 flex-col border-b pb-4 md:border-r md:border-b-0 md:pr-6"
										aria-label="Source record"
									>
										<div
											class="mb-3 flex items-center justify-between border-b pb-2"
										>
											<span
												class="text-xs font-semibold tracking-wider text-muted-foreground uppercase"
											>
												Incoming Record
											</span>
											<Badge variant="outline" size="xs"
												>Record {proposal.record_id.slice(0, 8)}</Badge
											>
										</div>
										<div class="flex flex-col gap-3 text-sm">
											<div>
												<span class="text-xs text-muted-foreground"
													>Title</span
												>
												<p class="font-medium">
													{displayDedupeTitle(proposal.source_title)}
												</p>
											</div>
											<div class="grid gap-2 text-xs">
												<div>
													<span class="text-muted-foreground">Year</span>
													<p class="font-medium">
														{formatDedupeYear(proposal.source_year)}
													</p>
												</div>
												<div>
													<span class="text-muted-foreground"
														>Identifiers</span
													>
													<ul
														aria-label="Source identifiers"
														class="flex flex-col gap-2 pt-1"
													>
														{#each identifierLabels(proposal.source_identifiers) as identifier (identifier)}
															<li class="font-mono break-all">
																{identifier}
															</li>
														{:else}<li class="text-muted-foreground">
																No identifiers available
															</li>{/each}
													</ul>
												</div>
											</div>
										</div>
									</section>

									<!-- Right Column: Candidate Record (Existing) -->
									<section
										class="flex min-w-0 flex-col pb-4"
										aria-label="Candidate report"
									>
										<div
											class="mb-3 flex items-center justify-between border-b pb-2"
										>
											<span
												class="text-xs font-semibold tracking-wider text-muted-foreground uppercase"
											>
												Existing Candidate
											</span>
											<Badge variant="secondary" size="xs"
												>Report {proposal.candidate_report_id?.slice(
													0,
													8
												)}</Badge
											>
										</div>
										<div class="flex flex-col gap-3 text-sm">
											<div>
												<span class="text-xs text-muted-foreground"
													>Title</span
												>
												<p class="font-medium">
													{displayDedupeTitle(proposal.candidate_title)}
												</p>
											</div>
											<div class="grid gap-2 text-xs">
												<div>
													<span class="text-muted-foreground">Year</span>
													<p class="font-medium">
														{formatDedupeYear(proposal.candidate_year)}
													</p>
												</div>
												<div>
													<span class="text-muted-foreground"
														>Identifiers</span
													>
													<ul
														aria-label="Candidate identifiers"
														class="flex flex-col gap-2 pt-1"
													>
														{#each identifierLabels(proposal.candidate_identifiers) as identifier (identifier)}
															<li class="font-mono break-all">
																{identifier}
															</li>
														{:else}<li class="text-muted-foreground">
																No identifiers available
															</li>{/each}
													</ul>
												</div>
											</div>
										</div>
									</section>
								</div>

								<!-- Conflict Banner if any -->
								{#if proposal.conflicting_identifier}
									<div
										class="mt-4 flex items-center gap-2 rounded-md border border-destructive/20 bg-destructive/10 p-3 text-xs text-destructive"
									>
										<span class="font-semibold">Conflict detected:</span>
										<span
											>Source and candidate have conflicting authoritative
											identifiers.</span
										>
									</div>
								{/if}
							</Card.Content>

							<!-- Card Footer with Review and Merge Actions -->
							<Card.Footer
								class="flex flex-wrap items-center justify-end gap-2 border-t bg-muted/10 p-3 sm:px-6"
							>
								<Tooltip.Provider>
									<Tooltip.Root>
										<Tooltip.Trigger>
											{#snippet child({ props })}
												<Button
													{...props}
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
													Not a duplicate
												</Button>
											{/snippet}
										</Tooltip.Trigger>
										<Tooltip.Content class="max-w-xs">
											Keeps this record as its own article. It enters
											Articles, screening and PRISMA, and it will not be
											proposed again.
										</Tooltip.Content>
									</Tooltip.Root>
								</Tooltip.Provider>
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
							<details class="disclosure">
								<summary>AI comparison and provenance</summary>
								<dl class="grid gap-2 p-4 text-xs">
									<div>
										<dt class="text-muted-foreground">Proposal</dt>
										<dd class="font-mono break-all">{proposal.id}</dd>
									</div>
									<div>
										<dt class="text-muted-foreground">Candidate report</dt>
										<dd class="font-mono break-all">
											{proposal.candidate_report_id}
										</dd>
									</div>
								</dl>
								<details class="px-4 pb-4 text-xs">
									<summary class="cursor-pointer text-muted-foreground"
										>Raw identifier provenance</summary
									>
									<div class="grid min-w-0 gap-3 pt-3 md:grid-cols-2">
										<div class="min-w-0">
											<p class="mb-2 font-semibold">Incoming record</p>
											<pre
												class="overflow-x-auto rounded bg-muted p-2">{JSON.stringify(
													proposal.source_identifiers,
													null,
													2
												)}</pre>
										</div>
										<div class="min-w-0">
											<p class="mb-2 font-semibold">Existing candidate</p>
											<pre
												class="overflow-x-auto rounded bg-muted p-2">{JSON.stringify(
													proposal.candidate_identifiers,
													null,
													2
												)}</pre>
										</div>
									</div>
								</details>
								<AiProposalReview
									{projectId}
									stage="dedupe"
									recordId={proposal.record_id}
									candidateReportId={proposal.candidate_report_id}
								/>
							</details>
						</Card.Root>
					{/each}
				</div>
			</div>
		{/if}
	</section>
</PageTemplate>
