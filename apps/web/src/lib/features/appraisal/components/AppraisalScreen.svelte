<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
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
		createListReportDocuments
	} from '$lib/api/generated/documents/documents';
	import { createListProjectReports } from '$lib/api/generated/reports/reports';
	import type {
		AiAppraisalPrefillProposalPayload,
		AiProposalDto,
		CompleteAppraisalRequest
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { fullTextUrlString } from '$lib/features/full-text/url';
	import { ReviewRunObserver } from '$lib/features/ai-assistance/review-run-observer.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import * as Alert from '$lib/components/ui/alert';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import {
		Brain,
		ClipboardCheck,
		FileCheck,
		FileSearch,
		Info,
		ListChecks,
		X
	} from '@lucide/svelte';
	import {
		appraisalAnswerValue,
		appraisalEvidenceLabel,
		mapAppraisalPrefillToFormState,
		serializeAppraisalPrefillReview
	} from '../ai-prefill';
	import type { AppraisalFormState } from '../form';
	import { parseAppraisalLocation, updateAppraisalLocation } from '../url';
	import AppraisalForm from './AppraisalForm.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	let error = $state<string | undefined>();

	const location = $derived(parseAppraisalLocation(page.url.searchParams));
	const reportId = $derived(location.reportId);
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

	async function selectReport(nextReportId: string): Promise<void> {
		const search = updateAppraisalLocation(page.url.searchParams, { reportId: nextReportId });
		let href: string = resolve('/projects/[projectId]/appraisal', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true });
	}

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
		error = undefined;
		if (activeAiProposal) {
			await decideAiProposal(activeAiProposal, 'accept', state);
			return;
		}
		try {
			await completeMutation.mutateAsync({ projectId, reportId, data: request });
			await queryClient.invalidateQueries({
				queryKey: getListReportAppraisalsQueryKey(projectId, reportId)
			});
		} catch (submitError) {
			error =
				submitError instanceof Error
					? submitError.message
					: 'Appraisal could not be completed.';
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

<div class="mx-auto flex min-h-full w-full max-w-[1480px] flex-col gap-5 p-4 md:gap-6 md:p-8">
	<header class="flex flex-col gap-4 lg:flex-row lg:items-end lg:justify-between">
		<div class="flex min-w-0 flex-col gap-2">
			<div
				class="flex flex-wrap items-center gap-2 text-xs font-semibold tracking-[0.12em] text-primary uppercase"
			>
				<FileSearch aria-hidden="true" /> Evidence workspace
				<span class="text-muted-foreground">/</span> appraisal
			</div>
			<h1 class="editorial-title text-4xl leading-none sm:text-5xl">Appraisal</h1>
			<p class="max-w-3xl text-sm leading-6 text-muted-foreground sm:text-base">
				Choose a report and exact definition version. AI pre-fills are editable proposals
				only and never change screening eligibility.
			</p>
		</div>
		<div class="flex flex-wrap items-center gap-2 lg:justify-end">
			<Badge variant="outline">Schema-driven</Badge>
			<Badge variant="secondary">Reviewer-led</Badge>
		</div>
	</header>

	{#if error}
		<Alert.Root variant="destructive" role="alert">
			<Info aria-hidden="true" />
			<Alert.Title>Appraisal could not be completed</Alert.Title>
			<Alert.Description>{error}</Alert.Description>
		</Alert.Root>
	{/if}

	<div
		class="grid min-w-0 gap-5 xl:grid-cols-[minmax(15rem,17rem)_minmax(18rem,20rem)_minmax(0,1fr)]"
	>
		<Card.Root class="border-primary/15">
			<Card.Header class="gap-2 border-b border-border/60 pb-4">
				<div class="flex items-center gap-2">
					<span
						class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
					>
						<FileSearch aria-hidden="true" />
					</span>
					<Card.Title>Report</Card.Title>
				</div>
				<Card.Description>URL selection is refresh-safe.</Card.Description>
			</Card.Header>
			<Card.Content class="pt-5">
				{#if reportsQuery.isPending}
					<div
						class="flex flex-col gap-3"
						aria-label="Loading reports"
						aria-live="polite"
					>
						<Skeleton class="h-5 w-2/3" />
						<Skeleton class="h-10 w-full" />
					</div>
				{:else if reportsQuery.error}
					<Alert.Root variant="destructive" role="alert">
						<Alert.Title>Reports unavailable</Alert.Title>
						<Alert.Description>{reportsQuery.error.message}</Alert.Description>
						<Alert.Action onclick={() => void reportsQuery.refetch()}
							>Retry</Alert.Action
						>
					</Alert.Root>
				{:else if reports.length === 0}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><FileSearch aria-hidden="true" /></Empty.Media>
						<Empty.Header>
							<Empty.Title>No reports available</Empty.Title>
							<Empty.Description
								>Add a report before starting an appraisal.</Empty.Description
							>
						</Empty.Header>
					</Empty.Root>
				{:else}
					<label for="appraisal-report" class="sr-only">Report to appraise</label>
					<select
						id="appraisal-report"
						class="h-10 w-full rounded-lg border border-border/80 bg-background px-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
						value={reportId ?? ''}
						onchange={(event) => {
							if (event.currentTarget instanceof HTMLSelectElement)
								void selectReport(event.currentTarget.value);
						}}
					>
						<option value="">Select a report</option>
						{#each reports as report (report.report_id)}<option value={report.report_id}
								>{report.title ?? report.report_id}</option
							>{/each}
					</select>
				{/if}
			</Card.Content>
		</Card.Root>

		<Card.Root class="border-primary/15">
			<Card.Header class="gap-2 border-b border-border/60 pb-4">
				<div class="flex items-center gap-2">
					<span
						class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
					>
						<ListChecks aria-hidden="true" />
					</span>
					<Card.Title>Definition</Card.Title>
				</div>
				<Card.Description>Generic API-provided schemas.</Card.Description>
			</Card.Header>
			<Card.Content class="flex flex-col gap-2 pt-5">
				{#if definitionsQuery.isPending}
					<div
						class="flex flex-col gap-3"
						aria-label="Loading appraisal definitions"
						aria-live="polite"
					>
						<Skeleton class="h-5 w-2/3" />
						<Skeleton class="h-16 w-full" />
					</div>
				{:else if definitionsQuery.error}
					<Alert.Root variant="destructive" role="alert">
						<Alert.Title>Definitions unavailable</Alert.Title>
						<Alert.Description>{definitionsQuery.error.message}</Alert.Description>
						<Alert.Action onclick={() => void definitionsQuery.refetch()}
							>Retry</Alert.Action
						>
					</Alert.Root>
				{:else if definitions.length === 0}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><ListChecks aria-hidden="true" /></Empty.Media>
						<Empty.Header>
							<Empty.Title>No appraisal definitions</Empty.Title>
							<Empty.Description
								>A versioned definition is required to begin.</Empty.Description
							>
						</Empty.Header>
					</Empty.Root>
				{:else}
					{#each definitions as definition (`${definition.id}:${definition.version}`)}
						<button
							type="button"
							class="rounded-xl border border-border/70 bg-background p-3 text-left shadow-xs transition outline-none hover:border-primary/40 hover:bg-muted/30 focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30 {selectedDefinition?.id ===
								definition.id && selectedDefinition.version === definition.version
								? 'border-primary bg-primary/5 ring-1 ring-primary/20'
								: ''}"
							aria-pressed={selectedDefinition?.id === definition.id &&
								selectedDefinition.version === definition.version}
							onclick={() => void selectDefinition(definition.id, definition.version)}
						>
							<span class="block font-medium">{definition.name}</span>
							<span class="text-xs text-muted-foreground"
								>v{definition.version} · {definition.domains.length} domains</span
							>
						</button>
					{/each}
				{/if}
			</Card.Content>
		</Card.Root>

		<Card.Root class="min-w-0 border-primary/15">
			<Card.Header class="gap-2 border-b border-border/60 pb-4">
				<div class="flex items-center gap-2">
					<span
						class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
					>
						<ClipboardCheck aria-hidden="true" />
					</span>
					<Card.Title>Complete appraisal</Card.Title>
				</div>
				<Card.Description
					>Evidence selectors use parsed PR8 document blocks from the selected report.</Card.Description
				>
			</Card.Header>
			<Card.Content class="pt-5">
				<Card.Root
					class="mb-6 border-primary/15 bg-muted/10"
					data-testid="ai-appraisal-prefill"
				>
					<Card.Header class="gap-3 border-b border-border/60 pb-4">
						<div class="flex flex-wrap items-center justify-between gap-2">
							<div class="flex items-center gap-2">
								<span
									class="flex size-7 items-center justify-center rounded-md bg-primary/10 text-primary"
								>
									<Brain aria-hidden="true" />
								</span>
								<Card.Title>AI appraisal pre-fill</Card.Title>
							</div>
							<Badge variant="outline">Reviewer required</Badge>
						</div>
						<Card.Description>
							Generate a grounded, version-pinned proposal for this report. Edit it in
							the form below before accepting. This assistance never changes screening
							eligibility.
						</Card.Description>
					</Card.Header>
					<Card.Content class="flex flex-col gap-4 pt-5">
						{#if !reportId || !selectedDefinition}
							<Empty.Root class="border-0 p-0">
								<Empty.Media variant="icon"><Info /></Empty.Media>
								<Empty.Header>
									<Empty.Title>Select report and definition</Empty.Title>
									<Empty.Description>
										An exact report and appraisal definition/version are
										required before generation.
									</Empty.Description>
								</Empty.Header>
							</Empty.Root>
						{:else}
							<div class="flex flex-wrap items-center gap-3">
								<Button
									type="button"
									variant="outline"
									onclick={() => void generateAiPrefill()}
									disabled={aiRequestPending}
									data-testid="generate-ai-prefill"
								>
									{#if generatePrefillMutation.isPending || reviewRun.isActive}<Spinner
											data-icon="inline-start"
										/>{/if}
									Generate pre-fill for {selectedDefinition.id} v{selectedDefinition.version}
								</Button>
								<span class="text-xs text-muted-foreground">
									Only pending proposals for this report and exact definition
									version are shown.
								</span>
							</div>

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

							{#if proposalsQuery.isPending}
								<div
									class="flex flex-col gap-3"
									aria-label="Loading AI appraisal proposal"
								>
									<Skeleton class="h-5 w-2/3" />
									<Skeleton class="h-20 w-full" />
								</div>
							{:else if !activeAiProposal}
								<Empty.Root class="border-0 p-0">
									<Empty.Media variant="icon"><Info /></Empty.Media>
									<Empty.Header>
										<Empty.Title>No pending AI pre-fill</Empty.Title>
										<Empty.Description>
											Generate a grounded proposal, then review every answer
											and evidence reference here.
										</Empty.Description>
									</Empty.Header>
								</Empty.Root>
							{:else if activeAiPayload?.kind === 'appraisal_prefill'}
								<div class="flex flex-wrap items-center gap-2">
									<Badge variant="secondary">Pending review</Badge>
									<span class="text-xs text-muted-foreground">
										{activeAiProposal?.provider} / {activeAiProposal?.model} · proposal
										{activeAiProposal?.id}
									</span>
								</div>
								{#if pendingAiProposals.length > 1}
									<div
										class="flex flex-wrap gap-2"
										aria-label="Pending appraisal proposals"
									>
										{#each pendingAiProposals as proposal (proposal.id)}
											<Button
												variant={proposal.id === activeAiProposal?.id
													? 'secondary'
													: 'outline'}
												size="sm"
												onclick={() => (selectedAiProposalId = proposal.id)}
											>
												{proposal.id.slice(0, 8)}
											</Button>
										{/each}
									</div>
								{/if}
								<div
									class="rounded-xl border border-primary/15 bg-background p-3 text-sm shadow-xs"
								>
									<p class="font-medium">Pinned appraisal context</p>
									<p class="mt-1 text-muted-foreground">
										Report {activeAiPayload.report_id} · definition {activeAiPayload.definition_id}
										v{activeAiPayload.definition_version}
									</p>
								</div>
								<div class="flex flex-col gap-3" data-testid="ai-prefill-proposal">
									{#each activeAiPayload.answers as answer (answer.question_id)}
										<div
											class="rounded-xl border border-border/70 bg-background p-3 shadow-xs"
										>
											<div
												class="flex flex-wrap items-center justify-between gap-2"
											>
												<span class="font-medium"
													>{questionLabel(answer.question_id)}</span
												>
												<Badge variant="secondary"
													>{answer.answer.kind}</Badge
												>
											</div>
											<p
												class="mt-2"
												data-testid={`ai-answer-${answer.question_id}`}
											>
												Suggested answer: {aiAnswerLabel(answer)}
											</p>
											<p class="mt-1 text-sm text-muted-foreground">
												{answer.rationale}
											</p>
											{#if answer.evidence.length}
												<div
													class="mt-3 flex flex-col gap-1"
													data-testid={`ai-evidence-list-${answer.question_id}`}
												>
													<span
														class="text-xs font-medium text-muted-foreground"
														>Grounding evidence</span
													>
													{#each answer.evidence as evidence (`${evidence.document_id}:${evidence.document_block_id}`)}
														<a
															class="inline-flex items-center gap-1 text-xs text-primary underline underline-offset-2"
															href={resolve(
																`/projects/${encodeURIComponent(projectId)}/screening/full-text${fullTextUrlString(
																	{
																		filter: 'all',
																		report: reportId,
																		page: evidence.page,
																		block: evidence.document_block_id
																	}
																)}`
															)}
															data-testid={`ai-evidence-link-${answer.question_id}`}
														>
															<FileSearch
																data-icon="inline-start"
															/>{appraisalEvidenceLabel(evidence)}
														</a>
													{/each}
												</div>
											{/if}
										</div>
									{/each}
								</div>
								<div class="grid gap-3 md:grid-cols-2">
									<div
										class="rounded-xl border border-border/70 bg-background p-3 text-sm shadow-xs"
									>
										<p class="font-medium">Domain judgments</p>
										<ul class="mt-2 flex flex-col gap-1 text-muted-foreground">
											{#each Object.entries(activeAiPayload.domain_judgments) as [domainId, judgment] (domainId)}
												<li>{domainId}: {judgment}</li>
											{/each}
										</ul>
									</div>
									<div
										class="rounded-xl border border-border/70 bg-background p-3 text-sm shadow-xs"
									>
										<p class="font-medium">Overall judgment</p>
										<p class="mt-2 text-muted-foreground">
											{activeAiPayload.overall_judgment}
										</p>
									</div>
								</div>
								<div class="flex flex-wrap items-center justify-between gap-3">
									<p class="text-xs text-muted-foreground">
										Edit the appraisal form below, then accept the reviewed
										proposal. Rejecting sends no reviewed payload.
									</p>
									<Button
										type="button"
										variant="destructive"
										onclick={() =>
											void decideAiProposal(
												activeAiProposal,
												'reject',
												undefined
											)}
										disabled={aiRequestPending}
										data-testid="reject-ai-prefill"
									>
										{#if decideProposalMutation.isPending}<Spinner
												data-icon="inline-start"
											/>{/if}
										<X data-icon="inline-start" />Reject pre-fill
									</Button>
								</div>
							{/if}
						{/if}
					</Card.Content>
				</Card.Root>
				{#if !reportId}
					<p class="text-sm text-muted-foreground">Select a report to begin.</p>
				{:else if !selectedDefinition}
					<p class="text-sm text-muted-foreground">
						Select an appraisal definition to begin.
					</p>
				{:else}
					{#key `${reportId}:${selectedDefinition.id}:${selectedDefinition.version}:${activeAiProposal?.id ?? 'manual'}`}
						<AppraisalForm
							definition={selectedDefinition}
							{blocks}
							{projectId}
							{reportId}
							initialState={activeAiPayload?.kind === 'appraisal_prefill'
								? mapAppraisalPrefillToFormState(activeAiPayload)
								: undefined}
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
			</Card.Content>
		</Card.Root>
	</div>

	{#if reportId}
		<Card.Root class="border-primary/15">
			<Card.Header class="gap-2 border-b border-border/60 pb-4">
				<div class="flex items-center gap-2">
					<span
						class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
					>
						<FileCheck aria-hidden="true" />
					</span>
					<Card.Title>Completed assessments</Card.Title>
				</div>
				<Card.Description
					>Immutable completion records with actor and evidence provenance.</Card.Description
				>
			</Card.Header>
			<Card.Content class="pt-5">
				{#if appraisalsQuery.isPending}
					<div
						class="flex flex-col gap-3"
						aria-label="Loading assessment history"
						aria-live="polite"
					>
						<Skeleton class="h-5 w-1/3" />
						<Skeleton class="h-14 w-full" />
					</div>
				{:else if appraisalsQuery.error}
					<Alert.Root variant="destructive" role="alert">
						<Alert.Title>Assessment history unavailable</Alert.Title>
						<Alert.Description>{appraisalsQuery.error.message}</Alert.Description>
					</Alert.Root>
				{:else if appraisals.length === 0}
					<Empty.Root class="border-0 p-0">
						<Empty.Media variant="icon"><FileCheck aria-hidden="true" /></Empty.Media>
						<Empty.Header>
							<Empty.Title>No completed assessments</Empty.Title>
							<Empty.Description
								>No completed assessments for this report.</Empty.Description
							>
						</Empty.Header>
					</Empty.Root>
				{:else}<div class="flex flex-col gap-3">
						{#each appraisals as appraisal (appraisal.id)}<div
								class="rounded-xl border border-border/70 bg-background p-4 text-sm shadow-xs"
							>
								<div class="flex flex-wrap justify-between gap-2">
									<span class="font-medium"
										>{appraisal.definition_id} v{appraisal.definition_version}</span
									><span class="text-xs text-muted-foreground"
										>{new Date(appraisal.completed_at).toLocaleString()} · {appraisal.actor_id}</span
									>
								</div>
								<p class="mt-1 text-xs text-muted-foreground">
									{appraisal.evidence.length} evidence references · assessment {appraisal.id}
								</p>
							</div>{/each}
					</div>{/if}
			</Card.Content>
		</Card.Root>
	{/if}
</div>
