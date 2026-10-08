<script lang="ts">
	import * as Resizable from '@deepref/ui/resizable';
	import { MediaQuery } from 'svelte/reactivity';
	import * as Alert from '@deepref/ui/alert';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { notifyError } from '$lib/features/notifications/toast';
	import {
		createDecideAiProposal,
		createGenerateAppraisalPrefillSuggestion,
		createListAiProposals
	} from '$lib/api/generated/ai/ai';
	import {
		createCompleteReportAppraisal,
		createListAppraisalDefinitions,
		createListReportAppraisals,
		getListReportAppraisalsQueryKey
	} from '$lib/api/generated/appraisal/appraisal';
	import {
		createListDocumentBlocks,
		createListFullTextScreeningQueue,
		createListReportDocuments
	} from '$lib/api/generated/documents/documents';
	import { createListProjectReports } from '$lib/api/generated/reports/reports';
	import { createGetReportStudyMembership } from '$lib/api/generated/studies/studies';
	import type {
		AiAppraisalPrefillProposalPayload,
		AiProposalDto,
		AppraisalAssessmentDto,
		CompleteAppraisalRequest
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { fullTextUrlString } from '$lib/features/full-text/url';
	import { ReviewRunObserver } from '$lib/features/ai-assistance/review-run-observer.svelte';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { Spinner } from '@deepref/ui/spinner';
	import { Brain, FileSearch, X } from '@lucide/svelte';
	import {
		appraisalAnswerValue,
		appraisalEvidenceLabel,
		appraisalEvidenceTechnical,
		mapAppraisalPrefillToFormState,
		serializeAppraisalPrefillReview
	} from '../ai-prefill';
	import type { AppraisalFormState } from '../form';
	import EvidenceLabel from '$lib/features/evidence/EvidenceLabel.svelte';
	import { humanizeCode } from '$lib/features/evidence/labels';
	import { parseAppraisalLocation, updateAppraisalLocation } from '../url';
	import AppraisalForm from './AppraisalForm.svelte';
	import AppraisalSummary from './AppraisalSummary.svelte';
	import { applicabilityStatus } from '../applicability';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	const wide = new MediaQuery('(min-width: 1024px)');
	// Unsaved answers per report, tool and proposal. The form is mounted in a different
	// layout branch when the window crosses the wide breakpoint; without this the answers
	// would be lost. Not reactive: it is only read when a form mounts.
	const drafts: Record<string, AppraisalFormState> = {};
	function forgetDrafts(report: string): void {
		for (const key of Object.keys(drafts)) if (key.startsWith(`${report}:`)) delete drafts[key];
	}

	const location = $derived(parseAppraisalLocation(page.url.searchParams));
	const reportId = $derived(location.reportId);
	const membershipQuery = createGetReportStudyMembership(
		() => projectId,
		() => reportId ?? '',
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const reportStudy = $derived(
		membershipQuery.data?.data &&
			typeof membershipQuery.data.data === 'object' &&
			'study_id' in membershipQuery.data.data
			? membershipQuery.data.data.study
			: undefined
	);
	const definitionsQuery = createListAppraisalDefinitions(() => projectId);
	const definitions = $derived(definitionsQuery.data?.data ?? []);
	const selectedDefinition = $derived(
		definitions.find(
			(definition) =>
				definition.id === location.definitionId &&
				definition.version === location.definitionVersion
		) ?? definitions[0]
	);
	const reportsQuery = createListProjectReports(
		() => projectId,
		() => ({ limit: 100 })
	);
	const documentsQuery = createListReportDocuments(
		() => projectId,
		() => reportId ?? '',
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const selectedDocumentId = $derived(documentsQuery.data?.data[0]?.id);
	const blocksQuery = createListDocumentBlocks(
		() => projectId,
		() => reportId ?? '',
		() => selectedDocumentId ?? '',
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(reportId && selectedDocumentId) } })
	);
	const appraisalsQuery = createListReportAppraisals(
		() => projectId,
		() => reportId ?? '',
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const proposalsQuery = createListAiProposals(
		() => projectId,
		() => ({
			status: 'pending',
			task_kind: 'appraisal_prefill',
			target_report_id: reportId,
			limit: 100
		}),
		() => ({ query: { enabled: Boolean(reportId && selectedDefinition) } })
	);
	const completeMutation = createCompleteReportAppraisal();
	const generatePrefillMutation = createGenerateAppraisalPrefillSuggestion();
	const decideProposalMutation = createDecideAiProposal();

	const reports = $derived(reportsQuery.data?.data.items ?? []);
	const blocks = $derived(blocksQuery.data?.data ?? []);
	const appraisals = $derived(appraisalsQuery.data?.data ?? []);
	const pendingAiProposals = $derived(
		(proposalsQuery.data?.data.items ?? []).filter(
			(
				proposal
			): proposal is AiProposalDto & {
				payload: AiAppraisalPrefillProposalPayload & { kind: 'appraisal_prefill' };
			} =>
				proposal.task_kind === 'appraisal_prefill' &&
				proposal.payload.kind === 'appraisal_prefill' &&
				proposal.payload.report_id === reportId &&
				proposal.payload.definition_id === selectedDefinition?.id &&
				proposal.payload.definition_version === selectedDefinition?.version
		)
	);
	// A finished assessment replaces the form with a read-only summary until the reviewer asks for a new one.
	let justCompleted = $state<AppraisalAssessmentDto | null>(null);
	let newAssessmentKey = $state<string | null>(null);
	const formKey = $derived(
		`${reportId}:${selectedDefinition?.id}:${selectedDefinition?.version}`
	);
	const completedAssessment = $derived.by((): AppraisalAssessmentDto | undefined => {
		if (!selectedDefinition) return undefined;
		return [...appraisals, ...(justCompleted ? [justCompleted] : [])]
			.filter(
				(item) =>
					item.report_id === reportId &&
					item.definition_id === selectedDefinition.id &&
					item.definition_version === selectedDefinition.version
			)
			.toSorted((a, b) => b.completed_at.localeCompare(a.completed_at))[0];
	});
	let selectedAiProposalId = $state<string | null>(null);
	let aiError = $state('');
	const reviewRun = new ReviewRunObserver(
		() => projectId,
		async () => {
			selectedAiProposalId = null;
			await proposalsQuery.refetch();
		}
	);
	const activeAiProposal = $derived(
		pendingAiProposals.find((proposal) => proposal.id === selectedAiProposalId) ??
			pendingAiProposals[0] ??
			null
	);
	const showSummary = $derived(
		Boolean(completedAssessment) && !activeAiProposal && newAssessmentKey !== formKey
	);
	const activeAiPayload = $derived(activeAiProposal?.payload ?? null);
	const aiRequestPending = $derived(
		generatePrefillMutation.isPending || decideProposalMutation.isPending || reviewRun.isActive
	);
	const aiStatus = $derived(
		aiError ||
			reviewRun.error ||
			proposalsQuery.error?.message ||
			generatePrefillMutation.error?.message ||
			decideProposalMutation.error?.message ||
			''
	);

	async function selectReport(nextReportId: string, replaceState = false): Promise<void> {
		const search = updateAppraisalLocation(page.url.searchParams, { reportId: nextReportId });
		let href: string = resolve('/projects/[projectId]/appraisal', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true, replaceState });
	}

	// Appraisal is for reports that survived full-text screening; the full list stays one click away.
	const fullTextQuery = createListFullTextScreeningQueue(
		() => projectId,
		() => ({ limit: 100 })
	);
	let showAllReports = $state(false);
	let showHistory = $state(false);
	const includedIds = $derived(
		new Set(
			(fullTextQuery.data?.data.items ?? [])
				.filter((item) => item.full_text_status === 'include')
				.map((item) => item.report_id)
		)
	);
	const candidates = $derived(
		showAllReports || fullTextQuery.isError
			? reports
			: reports.filter((report) => includedIds.has(report.report_id))
	);
	const selectedReport = $derived(reports.find((report) => report.report_id === reportId));

	$effect(() => {
		const first = candidates[0];
		if (!reportId && first && !fullTextQuery.isPending)
			void selectReport(first.report_id, true);
	});

	async function selectDefinition(definitionId: string, version: number): Promise<void> {
		const search = updateAppraisalLocation(page.url.searchParams, {
			definitionId,
			definitionVersion: version
		});
		let href: string = resolve('/projects/[projectId]/appraisal', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true });
	}

	function questionLabel(questionId: string): string {
		return (
			selectedDefinition?.domains
				.flatMap((domain) => domain.questions)
				.find((question) => question.id === questionId)?.label ?? questionId
		);
	}

	function aiAnswerLabel(answer: AiAppraisalPrefillProposalPayload['answers'][number]): string {
		const value = appraisalAnswerValue(answer.answer);
		return typeof value === 'boolean' ? (value ? 'Yes' : 'No') : String(value);
	}

	function aiDecisionError(error: unknown): string {
		if (error instanceof ApiError && error.status === 409) {
			return 'This AI proposal is stale. The appraisal queue was refreshed; review the current proposal before deciding again.';
		}
		if (error instanceof ApiError && error.status === 503) {
			return 'The configured AI provider is unavailable. Try again later or complete the appraisal manually.';
		}
		return error instanceof Error ? error.message : 'The AI appraisal decision failed.';
	}

	function statusCode(error: unknown): number | undefined {
		return error instanceof ApiError ? error.status : undefined;
	}

	const aiStatusCode = $derived(
		[
			statusCode(proposalsQuery.error),
			statusCode(generatePrefillMutation.error),
			statusCode(decideProposalMutation.error)
		].find((status) => status !== undefined)
	);

	async function refreshAfterDecision(): Promise<void> {
		await Promise.all([proposalsQuery.refetch(), appraisalsQuery.refetch()]);
	}

	async function generateAiPrefill(): Promise<void> {
		if (!reportId || !selectedDefinition) {
			aiError = 'Select a report and an exact appraisal definition/version first.';
			return;
		}
		aiError = '';
		try {
			const response = await generatePrefillMutation.mutateAsync({
				projectId,
				reportId,
				data: {
					definition_id: selectedDefinition.id,
					definition_version: selectedDefinition.version
				}
			});
			selectedAiProposalId = null;
			await reviewRun.observe(response.data);
		} catch (error) {
			aiError = aiDecisionError(error);
		}
	}

	async function decideAiProposal(
		proposal: AiProposalDto & {
			payload: AiAppraisalPrefillProposalPayload & { kind: 'appraisal_prefill' };
		},
		decision: 'accept' | 'reject',
		state: AppraisalFormState | undefined
	): Promise<void> {
		if (!reportId || !selectedDefinition) return;
		aiError = '';
		try {
			const reviewedPayload =
				decision === 'accept' && state
					? serializeAppraisalPrefillReview(
							selectedDefinition,
							reportId,
							state,
							proposal.payload,
							blocks
						)
					: undefined;
			await decideProposalMutation.mutateAsync({
				projectId,
				proposalId: proposal.id,
				data: {
					decision,
					reason:
						decision === 'accept'
							? 'Human reviewer accepted the edited AI appraisal prefill.'
							: 'Human reviewer rejected the AI appraisal prefill.',
					...(reviewedPayload ? { reviewed_payload: reviewedPayload } : {})
				}
			});
			selectedAiProposalId = null;
			await refreshAfterDecision();
			if (decision === 'accept') {
				newAssessmentKey = null;
				forgetDrafts(reportId);
			}
		} catch (error) {
			aiError = aiDecisionError(error);
			await Promise.all([proposalsQuery.refetch(), appraisalsQuery.refetch()]);
			if (decision === 'accept') throw error;
		}
	}

	async function submit(
		request: CompleteAppraisalRequest,
		state: AppraisalFormState
	): Promise<void> {
		if (!reportId) return;
		if (activeAiProposal) {
			await decideAiProposal(activeAiProposal, 'accept', state);
			return;
		}
		try {
			const response = await completeMutation.mutateAsync({
				projectId,
				reportId,
				data: request
			});
			justCompleted = response.data;
			newAssessmentKey = null;
			forgetDrafts(reportId);
			await queryClient.invalidateQueries({
				queryKey: getListReportAppraisalsQueryKey(projectId, reportId)
			});
		} catch (submitError) {
			notifyError('Appraisal could not be completed', submitError);
			throw submitError;
		}
	}
</script>

<svelte:head>
	<title>Appraisal · DeepRef</title>
	<meta
		name="description"
		content="Complete versioned, evidence-linked appraisals using a generic renderer."
	/>
</svelte:head>

{#snippet reportList()}
	<div class="flex items-center justify-between gap-2 px-4 pt-4 pb-2">
		<h2 class="text-sm font-semibold">
			{showAllReports ? 'All reports' : 'Included at full text'}
			<span class="font-normal text-muted-foreground tabular-nums">{candidates.length}</span>
		</h2>
		<Button variant="ghost" size="xs" onclick={() => (showAllReports = !showAllReports)}
			>{showAllReports ? 'Included only' : 'Show all'}</Button
		>
	</div>
	<div class="min-h-0 flex-1 overflow-y-auto">
		{#if reportsQuery.isPending}
			<div class="flex flex-col gap-2 p-2" aria-label="Loading reports" aria-live="polite">
				{#each { length: 4 }, index (index)}<Skeleton class="h-12 w-full" />{/each}
			</div>
		{:else if reportsQuery.error}
			<Alert.Root variant="destructive" role="alert" class="m-2">
				<Alert.Title>Reports unavailable</Alert.Title>
				<Alert.Description>{reportsQuery.error.message}</Alert.Description>
				<Alert.Action onclick={() => void reportsQuery.refetch()}>Retry</Alert.Action>
			</Alert.Root>
		{:else if candidates.length === 0}
			<p class="px-4 text-sm text-muted-foreground">
				{showAllReports
					? 'Add a report before starting an appraisal.'
					: 'No reports are included at full text yet.'}
			</p>
		{:else}
			<ol class="flex flex-col p-1.5" aria-label="Reports to appraise">
				{#each candidates as report (report.report_id)}
					{@const selected = report.report_id === reportId}
					<li>
						<button
							type="button"
							class={[
								'flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
								selected && 'bg-accent shadow-inset-accent'
							]}
							aria-current={selected ? 'true' : undefined}
							onclick={() => void selectReport(report.report_id)}
						>
							<span class={['line-clamp-2 leading-snug', selected && 'font-medium']}
								>{report.title ?? report.report_id}</span
							>
							{#if report.issued_year}<span
									class="text-xs text-muted-foreground tabular-nums"
									>{report.issued_year}</span
								>{/if}
						</button>
					</li>
				{/each}
			</ol>
		{/if}
	</div>
{/snippet}

{#snippet mobileReportPicker()}
	<select
		id="appraisal-report"
		aria-label="Report to appraise"
		class="h-10 w-full rounded-md border bg-background px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
		value={reportId ?? ''}
		onchange={(event) => void selectReport(event.currentTarget.value)}
	>
		<option value="">Select a report</option>
		{#each candidates as report (report.report_id)}<option value={report.report_id}
				>{report.title ?? report.report_id}</option
			>{/each}
	</select>
{/snippet}

{#snippet header()}
	<header class="flex flex-col gap-2">
		<h2 class="editorial-title text-xl leading-tight">
			{selectedReport?.title ?? 'Select a report'}
		</h2>
		<div class="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-muted-foreground">
			{#if definitionsQuery.isPending}
				<Skeleton class="h-5 w-48" />
			{:else if definitionsQuery.error}
				<span class="text-destructive" role="alert"
					>Frameworks unavailable · {definitionsQuery.error.message}</span
				>
				<Button variant="link" size="xs" onclick={() => void definitionsQuery.refetch()}
					>Retry</Button
				>
			{:else if definitions.length === 0}
				<span>No appraisal framework is configured for this project.</span>
			{:else}
				<label
					class="flex max-w-full min-w-0 items-center gap-1.5"
					for="appraisal-definition"
				>
					<span class="shrink-0">Framework</span>
					<select
						id="appraisal-definition"
						class="h-7 max-w-full min-w-0 truncate rounded-md bg-transparent px-1 text-sm text-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring sm:max-w-80"
						value={selectedDefinition
							? `${selectedDefinition.id}:${selectedDefinition.version}`
							: ''}
						onchange={(event) => {
							const definition = definitions.find(
								(item) => `${item.id}:${item.version}` === event.currentTarget.value
							);
							if (definition)
								void selectDefinition(definition.id, definition.version);
						}}
					>
						{#each definitions as definition (`${definition.id}:${definition.version}`)}
							<option value={`${definition.id}:${definition.version}`}
								>{definition.name} v{definition.version}</option
							>
						{/each}
					</select>
				</label>
			{/if}
			{#if reportId}
				<span aria-hidden="true">·</span>
				<button
					type="button"
					class="hover:text-foreground"
					aria-expanded={showHistory}
					onclick={() => (showHistory = !showHistory)}
					>{appraisalsQuery.isPending
						? 'Loading assessments…'
						: appraisals.length === 0
							? 'Not yet appraised'
							: `${appraisals.length} completed assessment${appraisals.length === 1 ? '' : 's'}`}</button
				>
			{/if}
		</div>
	</header>
{/snippet}

{#snippet applicabilityNotice()}
	{@const applicability =
		selectedDefinition && reportId && !membershipQuery.isPending
			? applicabilityStatus(selectedDefinition, reportStudy)
			: undefined}
	{#if applicability?.kind === 'mismatch' && selectedDefinition}
		<Alert.Root variant="warning" role="alert" data-testid="applicability-warning">
			<Alert.Title>This framework is not designed for this study</Alert.Title>
			<Alert.Description>
				{selectedDefinition.name} applies to {applicability.frameworkLabels.join(' or ')}.
				This report's study is classified as {applicability.studyLabel}. Choose a framework
				that fits the design, or reclassify the study on the Studies page.
			</Alert.Description>
		</Alert.Root>
	{:else if applicability?.kind === 'matches'}
		<p class="text-xs text-muted-foreground" data-testid="applicability-match">
			Study design: {applicability.studyLabel}. This framework applies.
		</p>
	{:else if applicability?.kind === 'unclassified'}
		<p class="text-xs text-muted-foreground" data-testid="applicability-unclassified">
			Study design not classified. Classify this report's study on the Studies page to check
			that the framework applies.
		</p>
	{/if}
{/snippet}

{#snippet history()}
	<section class="flex flex-col gap-2" aria-label="Completed assessments">
		{#if appraisalsQuery.error}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>Assessment history unavailable</Alert.Title>
				<Alert.Description>{appraisalsQuery.error.message}</Alert.Description>
			</Alert.Root>
		{:else}
			{#each appraisals as appraisal (appraisal.id)}
				<div
					class="flex flex-wrap items-baseline justify-between gap-2 border-b py-2 text-sm"
				>
					<span class="font-medium"
						>{appraisal.definition_id} v{appraisal.definition_version}</span
					>
					<span class="text-xs text-muted-foreground"
						>{new Date(appraisal.completed_at).toLocaleString()} · {appraisal.actor_id} ·
						{appraisal.evidence.length} evidence references · {appraisal.id}</span
					>
				</div>
			{:else}
				<p class="text-sm text-muted-foreground">
					No completed assessments for this report.
				</p>
			{/each}
		{/if}
	</section>
{/snippet}

{#snippet aiStrip()}
	<section class="flex flex-col gap-3" data-testid="ai-appraisal-prefill">
		{#if aiStatusCode === 503}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>AI provider unavailable</Alert.Title>
				<Alert.Description>
					{aiStatus ||
						'The configured provider is unavailable. Complete the appraisal manually or try again later.'}
				</Alert.Description>
			</Alert.Root>
		{:else if aiStatus}
			<Alert.Root variant="destructive" role="alert">
				<Alert.Title>AI appraisal needs attention</Alert.Title>
				<Alert.Description>{aiStatus}</Alert.Description>
			</Alert.Root>
		{/if}
		{#if activeAiProposal && activeAiPayload?.kind === 'appraisal_prefill'}
			<div class="flex flex-col gap-2 border-l-2 border-primary py-1 pl-4">
				<div class="flex flex-wrap items-center justify-between gap-2">
					<div class="min-w-0">
						<p class="text-sm font-medium">AI pre-fill loaded into the form below</p>
						<p class="text-xs text-muted-foreground">
							{activeAiProposal.provider} / {activeAiProposal.model} · check every answer
							and its evidence before accepting
						</p>
					</div>
					<Button
						type="button"
						variant="ghost"
						size="sm"
						onclick={() => void decideAiProposal(activeAiProposal, 'reject', undefined)}
						disabled={aiRequestPending}
						data-testid="reject-ai-prefill"
					>
						{#if decideProposalMutation.isPending}<Spinner
								data-icon="inline-start"
							/>{/if}
						<X data-icon="inline-start" />Discard pre-fill
					</Button>
				</div>
				{#if pendingAiProposals.length > 1}
					<div class="flex flex-wrap gap-1" aria-label="Pending appraisal proposals">
						{#each pendingAiProposals as proposal (proposal.id)}
							<Button
								variant={proposal.id === activeAiProposal.id
									? 'secondary'
									: 'ghost'}
								size="xs"
								onclick={() => (selectedAiProposalId = proposal.id)}
							>
								{proposal.id.slice(0, 8)}
							</Button>
						{/each}
					</div>
				{/if}
				<details class="text-sm">
					<summary class="cursor-pointer text-muted-foreground hover:text-foreground"
						>Rationale and evidence for {activeAiPayload.answers.length} answers · overall
						{activeAiPayload.overall_judgment}</summary
					>
					<ol class="flex flex-col gap-4 pt-3" data-testid="ai-prefill-proposal">
						{#each activeAiPayload.answers as answer (answer.question_id)}
							<li class="flex flex-col gap-1">
								<span class="font-medium">{questionLabel(answer.question_id)}</span>
								<span data-testid={`ai-answer-${answer.question_id}`}
									>Suggested answer: {aiAnswerLabel(answer)}</span
								>
								<span class="text-muted-foreground">{answer.rationale}</span>
								{#if answer.evidence.length}
									<span
										class="flex flex-wrap gap-x-3"
										data-testid={`ai-evidence-list-${answer.question_id}`}
									>
										{#each answer.evidence as evidence (`${evidence.document_id}:${evidence.document_block_id}`)}
											<a
												class="inline-flex max-w-full min-w-0 items-start gap-1 text-xs text-primary underline underline-offset-2"
												href={resolve(
													`/projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString(
														{
															filter: 'all',
															report: reportId ?? null,
															page: evidence.page,
															block: evidence.document_block_id
														}
													)}`
												)}
												data-testid={`ai-evidence-link-${answer.question_id}`}
											>
												<FileSearch
													data-icon="inline-start"
												/><EvidenceLabel
													label={appraisalEvidenceLabel(
														evidence,
														blocks.find(
															(block) =>
																block.id ===
																evidence.document_block_id
														)?.text
													)}
													technical={appraisalEvidenceTechnical(evidence)}
												/>
											</a>
										{/each}
									</span>
								{/if}
							</li>
						{/each}
						<li class="text-muted-foreground">
							Domain judgments: {Object.entries(activeAiPayload.domain_judgments)
								.map(
									([domainId, judgment]) =>
										`${humanizeCode(domainId)}: ${humanizeCode(judgment)}`
								)
								.join(' · ')}
						</li>
					</ol>
				</details>
			</div>
		{:else if reportId && selectedDefinition && !showSummary}
			<div class="flex flex-wrap items-center gap-3">
				<Button
					type="button"
					variant="outline"
					size="sm"
					onclick={() => void generateAiPrefill()}
					disabled={aiRequestPending || proposalsQuery.isPending}
					data-testid="generate-ai-prefill"
				>
					{#if generatePrefillMutation.isPending || reviewRun.isActive}<Spinner
							data-icon="inline-start"
						/>{:else}<Brain data-icon="inline-start" />{/if}
					Pre-fill with AI
				</Button>
				<span class="text-xs text-muted-foreground"
					>Suggestions fill the form for your review; nothing is saved until you accept.</span
				>
			</div>
		{/if}
	</section>
{/snippet}

{#snippet assessment()}
	{#if !reportId}
		<p class="text-sm text-muted-foreground">
			{candidates.length === 0 ? '' : 'Choose a report to appraise.'}
		</p>
	{:else if selectedDefinition && showSummary && completedAssessment}
		<AppraisalSummary
			definition={selectedDefinition}
			assessment={completedAssessment}
			onStartNew={() => (newAssessmentKey = formKey)}
		/>
	{:else if selectedDefinition}
		{@const draftKey = `${reportId}:${selectedDefinition.id}:${selectedDefinition.version}:${activeAiProposal?.id ?? 'manual'}`}
		{#key draftKey}
			<AppraisalForm
				definition={selectedDefinition}
				{blocks}
				{projectId}
				{reportId}
				initialState={drafts[draftKey] ??
					(activeAiPayload?.kind === 'appraisal_prefill'
						? mapAppraisalPrefillToFormState(activeAiPayload)
						: undefined)}
				onStateChange={(state) => (drafts[draftKey] = state)}
				originalPrefill={activeAiPayload?.kind === 'appraisal_prefill'
					? activeAiPayload
					: undefined}
				submitLabel={activeAiPayload?.kind === 'appraisal_prefill'
					? 'Accept reviewed AI pre-fill'
					: undefined}
				onSubmit={submit}
			/>
		{/key}
	{/if}
{/snippet}

{#snippet evidence()}
	<aside class="flex flex-col gap-4" aria-label="Source evidence">
		<h3 class="text-sm font-semibold">Source evidence</h3>
		{#if !reportId}
			<p class="text-sm text-muted-foreground">
				The selected report's parsed text appears here.
			</p>
		{:else if documentsQuery.error}
			<Alert.Root variant="destructive">
				<Alert.Title>Documents unavailable</Alert.Title>
				<Alert.Description>{documentsQuery.error.message}</Alert.Description>
			</Alert.Root>
		{:else if documentsQuery.isPending || (selectedDocumentId && blocksQuery.isPending)}
			<Skeleton class="h-24 w-full" />
		{:else if blocksQuery.error}
			<Alert.Root variant="destructive"
				><Alert.Title>Evidence unavailable</Alert.Title><Alert.Description
					>{blocksQuery.error.message}</Alert.Description
				></Alert.Root
			>
		{:else}
			{#each blocks as block (block.id)}
				<article class="flex flex-col gap-1 border-b pb-4">
					<a
						class="text-xs text-primary underline"
						href={resolve(
							`/projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString({ filter: 'all', report: reportId, page: block.page_number, block: block.id })}`
						)}>Page {block.page_number}</a
					>
					<p class="text-sm leading-6 whitespace-pre-wrap">{block.text}</p>
				</article>
			{:else}
				<div class="flex flex-col items-start gap-2">
					<p class="text-sm text-muted-foreground">
						No parsed text yet. Attach this report's PDF to cite evidence in the
						assessment.
					</p>
					<Button
						variant="outline"
						size="sm"
						href={resolve(
							`/projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString({ filter: 'all', report: reportId, page: null, block: null })}`
						)}>Open in full text</Button
					>
				</div>
			{/each}
		{/if}
	</aside>
{/snippet}

{#snippet center()}
	{@render header()}
	{@render applicabilityNotice()}
	{#if showHistory && reportId}{@render history()}{/if}
	{@render aiStrip()}
	{@render assessment()}
{/snippet}

{#if wide.current}
	<div class="h-full min-h-0" data-testid="appraisal-page">
		<Resizable.PaneGroup
			direction="horizontal"
			class="h-full"
			autoSaveId="deepref:appraisal-layout"
		>
			<Resizable.Pane order={1} defaultSize={20} minSize={14} maxSize={34}>
				<nav class="flex h-full min-h-0 flex-col" aria-label="Reports">
					{@render reportList()}
				</nav>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={2} defaultSize={50} minSize={34}>
				<div class="flex h-full flex-col gap-6 overflow-y-auto px-8 py-6">
					{@render center()}
				</div>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={3} defaultSize={30} minSize={18} maxSize={45}>
				<div class="h-full overflow-y-auto px-5 py-6">{@render evidence()}</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
{:else}
	<PageTemplate testId="appraisal-page" containerClass="gap-6">
		{@render mobileReportPicker()}
		{@render center()}
		<details class="disclosure">
			<summary>Source evidence</summary>
			<div class="pt-3">{@render evidence()}</div>
		</details>
	</PageTemplate>
{/if}
