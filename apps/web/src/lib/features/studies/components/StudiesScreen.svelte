<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import {
		createDecideAiProposal,
		createGenerateStudyGroupingSuggestion,
		createListAiProposals,
		getListAiProposalsQueryKey
	} from '$lib/api/generated/ai/ai';
	import type {
		AiProposalDecisionInput,
		AiProposalDto,
		AiStudyGroupingEvidenceDto,
		AiStudyGroupingFieldDto,
		AiStudyGroupingProposalPayload
	} from '$lib/api/generated/models';
	import {
		createClassifyProjectStudy,
		createCreateProjectStudy,
		createGetProjectStudy,
		createGetReportStudyMembership,
		createListProjectStudies,
		createListProjectStudyHistory,
		createPutReportStudyMembership,
		createRenameProjectStudy,
		getGetProjectStudyQueryKey,
		getGetReportStudyMembershipQueryKey,
		getListProjectStudiesQueryKey,
		getListProjectStudyHistoryQueryKey
	} from '$lib/api/generated/studies/studies';
	import {
		StudyReportRoleInput,
		type StudyReportRoleInput as StudyReportRole
	} from '$lib/api/generated/models/studyReportRoleInput';
	import { createListProjectReports } from '$lib/api/generated/reports/reports';
	import { ApiError } from '$lib/api/custom-fetch';
	import { useQueryClient } from '@tanstack/svelte-query';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { Input } from '$lib/components/ui/input';
	import * as Select from '$lib/components/ui/select';
	import { Separator } from '$lib/components/ui/separator';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Brain, Check, Info, X } from '@lucide/svelte';
	import { parseStudyLocation, updateStudyLocation } from '../url';
	import StudyClassificationForm from './StudyClassificationForm.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	let newTitle = $state('');
	let renameTitle = $state('');
	let reportId = $state('');
	let role = $state<StudyReportRole>(StudyReportRoleInput.report_of_study);
	let formError = $state<string | undefined>();
	let groupingError = $state('');
	let groupingErrorStatus = $state<number | null>(null);
	let groupingAction = $state<'generate' | 'accept' | 'reject' | null>(null);
	let pendingGroupingProposalId = $state<string | null>(null);

	const location = $derived(parseStudyLocation(page.url.searchParams));
	const selectedStudyId = $derived(location.studyId);
	const studiesQuery = createListProjectStudies(
		() => projectId,
		() => ({ limit: 100 })
	);
	const reportsQuery = createListProjectReports(
		() => projectId,
		() => ({ limit: 100 })
	);
	const studyQuery = createGetProjectStudy(
		() => projectId,
		() => selectedStudyId ?? '',
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const membershipQuery = createGetReportStudyMembership(
		() => projectId,
		() => reportId,
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const historyQuery = createListProjectStudyHistory(
		() => projectId,
		() => selectedStudyId ?? '',
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const groupingProposalsQuery = createListAiProposals(
		() => projectId,
		() => ({
			status: 'pending',
			task_kind: 'study_grouping',
			limit: 1,
			...(reportId ? { target_report_id: reportId } : {})
		}),
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const createMutation = createCreateProjectStudy();
	const renameMutation = createRenameProjectStudy();
	const classifyMutation = createClassifyProjectStudy();
	const membershipMutation = createPutReportStudyMembership();
	const generateGroupingMutation = createGenerateStudyGroupingSuggestion();
	const decideGroupingMutation = createDecideAiProposal();

	const studies = $derived(studiesQuery.data?.data.items ?? []);
	const reports = $derived(reportsQuery.data?.data.items ?? []);
	const selectedReport = $derived(reports.find((report) => report.report_id === reportId));
	const selectedStudy = $derived(studyQuery.data?.data);
	const selectedMembership = $derived(
		membershipQuery.data?.data &&
			typeof membershipQuery.data.data === 'object' &&
			'study_id' in membershipQuery.data.data
			? membershipQuery.data.data
			: undefined
	);
	const history = $derived(historyQuery.data?.data ?? []);
	const groupingProposals = $derived(groupingProposalsQuery.data?.data.items ?? []);
	const activeGroupingProposal = $derived(
		groupingProposals.find((proposal) => proposal.payload.kind === 'study_grouping')
	);
	const activeGroupingPayload = $derived(
		activeGroupingProposal ? groupingPayload(activeGroupingProposal) : undefined
	);
	const groupingQueryError = $derived(groupingProposalsQuery.error?.message ?? '');
	const groupingMutationError = $derived(
		generateGroupingMutation.error?.message ?? decideGroupingMutation.error?.message ?? ''
	);
	const groupingErrorMessage = $derived(
		groupingError || groupingQueryError || groupingMutationError
	);
	const groupingConflict = $derived(
		groupingErrorStatus === 409 ||
			isApiErrorWithStatus(groupingProposalsQuery.error, 409) ||
			isApiErrorWithStatus(generateGroupingMutation.error, 409) ||
			isApiErrorWithStatus(decideGroupingMutation.error, 409)
	);
	const groupingProviderUnavailable = $derived(
		groupingErrorStatus === 503 ||
			isApiErrorWithStatus(groupingProposalsQuery.error, 503) ||
			isApiErrorWithStatus(generateGroupingMutation.error, 503) ||
			isApiErrorWithStatus(decideGroupingMutation.error, 503)
	);
	const designs = [
		{ value: 'rct', label: 'Randomized controlled trial' },
		{ value: 'non_randomized_intervention', label: 'Non-randomized intervention' },
		{ value: 'cohort', label: 'Cohort' },
		{ value: 'case_control', label: 'Case-control' },
		{ value: 'cross_sectional', label: 'Cross-sectional' },
		{ value: 'diagnostic_accuracy', label: 'Diagnostic accuracy' },
		{ value: 'prediction_model', label: 'Prediction model' },
		{ value: 'qualitative', label: 'Qualitative' },
		{ value: 'systematic_review', label: 'Systematic review' },
		{ value: 'case_series', label: 'Case series' }
	] as const;

	function isApiErrorWithStatus(error: Error | null | undefined, status: number): boolean {
		return error instanceof ApiError && error.status === status;
	}

	function groupingPayload(proposal: AiProposalDto): AiStudyGroupingProposalPayload | undefined {
		switch (proposal.payload.kind) {
			case 'study_grouping':
				return proposal.payload;
			case 'screening':
			case 'duplicate':
			case 'appraisal_prefill':
			case 'data_extraction':
				return undefined;
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function groupingFieldLabel(field: AiStudyGroupingFieldDto): string {
		switch (field) {
			case 'title':
				return 'Title';
			case 'abstract':
				return 'Abstract';
			case 'publication_year':
				return 'Publication year';
			case 'first_author':
				return 'First author';
			default: {
				const exhaustive: never = field;
				return exhaustive;
			}
		}
	}

	function studyLabel(studyId: string): string {
		return studies.find((study) => study.id === studyId)?.title ?? studyId;
	}

	function groupingEvidenceSubject(evidence: AiStudyGroupingEvidenceDto): string {
		switch (evidence.kind) {
			case 'report_metadata':
				return `Report ${evidence.report_id}`;
			case 'study_metadata':
				return `Study ${studyLabel(evidence.study_id)}`;
			case 'study_report_metadata':
				return `Study ${studyLabel(evidence.study_id)} · report ${evidence.report_id}`;
			default: {
				const exhaustive: never = evidence;
				return exhaustive;
			}
		}
	}

	function affectedStudyIds(payload: AiStudyGroupingProposalPayload): string[] {
		const ids: string[] = [];
		const addStudyId = (studyId: string | null | undefined): void => {
			if (studyId && !ids.includes(studyId)) ids.push(studyId);
		};
		addStudyId(selectedStudyId);
		addStudyId(payload.expected_previous_study_id);
		if (payload.choice.kind === 'existing_study') addStudyId(payload.choice.study_id);
		return ids;
	}

	async function refreshGroupingQueries(payload: AiStudyGroupingProposalPayload): Promise<void> {
		await queryClient.invalidateQueries({
			queryKey: getListAiProposalsQueryKey(projectId)
		});
		await refreshStudy();
		await Promise.all(
			affectedStudyIds(payload).map(async (studyId) => {
				if (studyId === selectedStudyId) return;
				await queryClient.invalidateQueries({
					queryKey: getGetProjectStudyQueryKey(projectId, studyId)
				});
				await queryClient.invalidateQueries({
					queryKey: getListProjectStudyHistoryQueryKey(projectId, studyId)
				});
			})
		);
		await groupingProposalsQuery.refetch();
	}

	async function generateGrouping(): Promise<void> {
		if (!reportId || groupingAction) return;
		groupingError = '';
		groupingErrorStatus = null;
		groupingAction = 'generate';
		try {
			await generateGroupingMutation.mutateAsync({ projectId, reportId });
			await groupingProposalsQuery.refetch();
		} catch (error) {
			groupingError =
				error instanceof Error ? error.message : 'Study grouping suggestion failed.';
			groupingErrorStatus = error instanceof ApiError ? error.status : null;
			await groupingProposalsQuery.refetch();
		} finally {
			groupingAction = null;
		}
	}

	async function decideGrouping(decision: AiProposalDecisionInput): Promise<void> {
		if (!activeGroupingProposal || !activeGroupingPayload || pendingGroupingProposalId) return;
		const proposal = activeGroupingProposal;
		const payload = activeGroupingPayload;
		pendingGroupingProposalId = proposal.id;
		groupingError = '';
		groupingErrorStatus = null;
		groupingAction = decision === 'accept' ? 'accept' : 'reject';
		try {
			await decideGroupingMutation.mutateAsync({
				projectId,
				proposalId: proposal.id,
				data: {
					decision,
					reason: `Human reviewer ${decision === 'accept' ? 'accepted' : 'rejected'} study grouping suggestion.`
				}
			});
			await refreshGroupingQueries(payload);
		} catch (error) {
			groupingError =
				error instanceof ApiError && error.status === 409
					? 'This study or report membership changed elsewhere. The proposal remains pending; refresh and review it again.'
					: error instanceof Error
						? error.message
						: 'The study grouping decision failed.';
			groupingErrorStatus = error instanceof ApiError ? error.status : null;
			await groupingProposalsQuery.refetch();
		} finally {
			pendingGroupingProposalId = null;
			groupingAction = null;
		}
	}

	async function selectStudy(studyId: string): Promise<void> {
		const search = updateStudyLocation(page.url.searchParams, { studyId, reportId: '' });
		let href: string = resolve('/projects/[projectId]/studies', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true });
	}

	async function createStudy(): Promise<void> {
		formError = undefined;
		try {
			const response = await createMutation.mutateAsync({
				projectId,
				data: { title: newTitle }
			});
			newTitle = '';
			await queryClient.invalidateQueries({
				queryKey: getListProjectStudiesQueryKey(projectId)
			});
			await selectStudy(response.data.id);
		} catch (error) {
			formError = error instanceof Error ? error.message : 'Study could not be created.';
		}
	}

	async function renameStudy(): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
		formError = undefined;
		try {
			await renameMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				data: { title: renameTitle, expected_revision: selectedStudy.revision }
			});
			renameTitle = '';
			await refreshStudy();
		} catch (error) {
			formError = error instanceof Error ? error.message : 'Study could not be renamed.';
		}
	}

	async function classify(request: {
		design: string;
		physiotherapy: boolean;
		exposure: boolean;
		prediction_or_ai: boolean;
	}): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
		formError = undefined;
		try {
			await classifyMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				data: {
					design: request.design,
					expected_revision: selectedStudy.revision,
					physiotherapy: request.physiotherapy,
					exposure: request.exposure,
					prediction_or_ai: request.prediction_or_ai
				}
			});
			await refreshStudy();
		} catch (error) {
			formError = error instanceof Error ? error.message : 'Study classification failed.';
		}
	}

	async function assignReport(): Promise<void> {
		if (!selectedStudyId || !selectedStudy || !reportId) return;
		formError = undefined;
		const previousStudyId = selectedMembership?.study_id;
		const expectedPreviousRevision =
			previousStudyId && previousStudyId !== selectedStudyId
				? selectedMembership?.study_revision
				: undefined;
		try {
			await membershipMutation.mutateAsync({
				projectId,
				reportId,
				data: {
					study_id: selectedStudyId,
					role,
					expected_revision: selectedStudy.revision,
					expected_previous_study_revision: expectedPreviousRevision
				}
			});
			await refreshStudy(previousStudyId);
			reportId = '';
		} catch (error) {
			formError = error instanceof Error ? error.message : 'Report could not be assigned.';
		}
	}

	async function unassignReport(selectedReportId: string): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
		formError = undefined;
		try {
			await membershipMutation.mutateAsync({
				projectId,
				reportId: selectedReportId,
				data: { study_id: null, expected_revision: selectedStudy.revision }
			});
			await refreshStudy(undefined, selectedReportId);
		} catch (error) {
			formError = error instanceof Error ? error.message : 'Report could not be unassigned.';
		}
	}

	async function refreshStudy(
		sourceStudyId: string | undefined = undefined,
		changedReportId = reportId
	): Promise<void> {
		await queryClient.invalidateQueries({ queryKey: getListProjectStudiesQueryKey(projectId) });
		if (selectedStudyId) {
			await queryClient.invalidateQueries({
				queryKey: getGetProjectStudyQueryKey(projectId, selectedStudyId)
			});
			await queryClient.invalidateQueries({
				queryKey: getListProjectStudyHistoryQueryKey(projectId, selectedStudyId)
			});
			if (sourceStudyId && sourceStudyId !== selectedStudyId) {
				await queryClient.invalidateQueries({
					queryKey: getGetProjectStudyQueryKey(projectId, sourceStudyId)
				});
				await queryClient.invalidateQueries({
					queryKey: getListProjectStudyHistoryQueryKey(projectId, sourceStudyId)
				});
			}
		}
		if (changedReportId) {
			await queryClient.invalidateQueries({
				queryKey: getGetReportStudyMembershipQueryKey(projectId, changedReportId)
			});
		}
	}
</script>

<svelte:head>
	<title>Studies · DeepRef</title>
	<meta
		name="description"
		content="Group reports into reversible study aggregates and classify their design."
	/>
</svelte:head>

<main class="mx-auto flex max-w-7xl flex-col gap-6 p-6">
	<div>
		<p class="text-sm text-muted-foreground">Evidence identity</p>
		<h1 class="text-3xl font-semibold tracking-tight">Studies</h1>
		<p class="mt-2 max-w-3xl text-muted-foreground">
			Group reports from one investigation so follow-ups and safety analyses are not counted
			as independent evidence.
		</p>
	</div>

	{#if formError}
		<p
			class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive"
			role="alert"
		>
			{formError}
		</p>
	{/if}

	<div class="grid gap-6 lg:grid-cols-[20rem_1fr]">
		<Card.Root>
			<Card.Header>
				<Card.Title>Study groups</Card.Title>
				<Card.Description>{studies.length} groups in this project</Card.Description>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4">
				<form
					class="flex flex-col gap-2"
					onsubmit={(event) => {
						event.preventDefault();
						void createStudy();
					}}
				>
					<label for="new-study-title" class="text-sm font-medium">New study title</label>
					<Input
						id="new-study-title"
						bind:value={newTitle}
						placeholder="e.g. AMBIENT-AI Trial"
						required
					/>
					<Button type="submit" disabled={createMutation.isPending}>Create study</Button>
				</form>
				<Separator />
				<div class="flex flex-col gap-2" aria-live="polite">
					{#if studiesQuery.isPending}
						<p class="text-sm text-muted-foreground">Loading studies…</p>
					{:else if studies.length === 0}
						<p class="text-sm text-muted-foreground">No study groups yet.</p>
					{:else}
						{#each studies as study (study.id)}
							<button
								type="button"
								class="flex flex-col gap-1 rounded-md border p-3 text-left transition hover:bg-muted/50 {selectedStudyId ===
								study.id
									? 'border-primary bg-muted/50'
									: ''}"
								onclick={() => void selectStudy(study.id)}
							>
								<span class="font-medium">{study.title}</span>
								<span class="text-xs text-muted-foreground"
									>Open to inspect membership · revision {study.revision}</span
								>
							</button>
						{/each}
					{/if}
				</div>
			</Card.Content>
		</Card.Root>

		{#if selectedStudy}
			<div class="flex flex-col gap-6">
				<Card.Root>
					<Card.Header>
						<div class="flex flex-wrap items-start justify-between gap-3">
							<div>
								<Card.Title>{selectedStudy.title}</Card.Title>
								<Card.Description
									>Revision {selectedStudy.revision} · changes are audited and reversible</Card.Description
								>
							</div>
							{#if selectedStudy.design_label}<Badge variant="secondary"
									>{selectedStudy.design_label}</Badge
								>{/if}
						</div>
					</Card.Header>
					<Card.Content class="flex flex-col gap-6">
						<div class="grid gap-4 md:grid-cols-3">
							<form
								class="flex flex-col gap-2"
								onsubmit={(event) => {
									event.preventDefault();
									void renameStudy();
								}}
							>
								<label for="rename-study-title" class="text-sm font-medium"
									>Rename</label
								>
								<Input
									id="rename-study-title"
									bind:value={renameTitle}
									placeholder={selectedStudy.title}
									required
								/>
								<Button
									type="submit"
									variant="outline"
									disabled={renameMutation.isPending}>Save title</Button
								>
							</form>
							{#key `${selectedStudy.id}:${selectedStudy.revision}`}
								<StudyClassificationForm
									study={selectedStudy}
									{designs}
									disabled={classifyMutation.isPending}
									onSubmit={classify}
								/>
							{/key}
							<div class="rounded-md border bg-muted/20 p-3 text-sm">
								<p class="font-medium">Suggested tools</p>
								<p class="mt-1 text-xs text-muted-foreground">
									Guidance only; never completes an appraisal automatically.
								</p>
								<div class="mt-3 flex flex-wrap gap-2">
									{#each selectedStudy.tool_suggestions as suggestion (suggestion.tool)}<Badge
											variant="outline"
											title={suggestion.rationale}>{suggestion.tool}</Badge
										>{/each}
								</div>
							</div>
						</div>

						<Separator />
						<form
							class="grid gap-3 md:grid-cols-[1fr_12rem_auto]"
							onsubmit={(event) => {
								event.preventDefault();
								void assignReport();
							}}
						>
							<div class="flex flex-col gap-2">
								<label for="study-report" class="text-sm font-medium"
									>Assign included report</label
								><Select.Root type="single" bind:value={reportId}
									><Select.Trigger id="study-report"
										>{selectedReport?.title ??
											(reportId || 'Choose report')}</Select.Trigger
									><Select.Content
										><Select.Group
											>{#each reports as report (report.report_id)}<Select.Item
													value={report.report_id}
													label={report.title ?? report.report_id}
													>{report.title ?? report.report_id}</Select.Item
												>{/each}</Select.Group
										></Select.Content
									></Select.Root
								>
							</div>
							<div class="flex flex-col gap-2">
								<label for="report-role" class="text-sm font-medium"
									>Report role</label
								><Select.Root type="single" bind:value={role}
									><Select.Trigger id="report-role">{role}</Select.Trigger
									><Select.Content
										><Select.Group
											>{#each ['report_of_study', 'protocol', 'primary_outcome', 'safety_analysis', 'economic_analysis', 'follow_up'] as value (value)}<Select.Item
													{value}
													label={value}>{value}</Select.Item
												>{/each}</Select.Group
										></Select.Content
									></Select.Root
								>
							</div>
							<Button
								type="submit"
								class="self-end"
								disabled={!reportId ||
									membershipMutation.isPending ||
									membershipQuery.isPending ||
									membershipQuery.isFetching}>Assign / move</Button
							>
						</form>

						<Card.Root data-testid="study-grouping-assistance">
							<Card.Header class="gap-3">
								<div class="flex flex-wrap items-center justify-between gap-2">
									<div class="flex items-center gap-2">
										<Brain aria-hidden="true" class="size-4" />
										<Card.Title>Study grouping assistance</Card.Title>
									</div>
									<Badge variant="outline">Proposal only</Badge>
								</div>
								<Card.Description>
									AI compares report metadata with existing studies and proposes a
									reversible grouping. A reviewer must approve it; grouping never
									changes screening or appraisal decisions.
								</Card.Description>
							</Card.Header>
							<Card.Content class="flex flex-col gap-4">
								{#if groupingErrorMessage}
									<Alert.Root variant="destructive" role="alert">
										<Alert.Title>
											{groupingConflict
												? 'Study data changed elsewhere'
												: groupingProviderUnavailable
													? 'AI provider unavailable'
													: 'Study grouping suggestion unavailable'}
										</Alert.Title>
										<Alert.Description>{groupingErrorMessage}</Alert.Description
										>
									</Alert.Root>
								{:else if !reportId}
									<Empty.Root class="border-0 p-0">
										<Empty.Media variant="icon"><Info /></Empty.Media>
										<Empty.Header>
											<Empty.Title>Select a report</Empty.Title>
											<Empty.Description>
												Choose a report above to request a grounded study
												grouping suggestion.
											</Empty.Description>
										</Empty.Header>
									</Empty.Root>
								{:else if groupingProposalsQuery.isPending}
									<div
										class="flex flex-col gap-3"
										aria-label="Loading study grouping suggestion"
									>
										<Skeleton class="h-5 w-2/3" />
										<Skeleton class="h-20 w-full" />
									</div>
								{:else if !activeGroupingProposal || !activeGroupingPayload}
									<Empty.Root class="border-0 p-0">
										<Empty.Media variant="icon"><Info /></Empty.Media>
										<Empty.Header>
											<Empty.Title>No pending grouping suggestion</Empty.Title
											>
											<Empty.Description>
												Request a suggestion to compare this report with the
												project’s study groups.
											</Empty.Description>
										</Empty.Header>
									</Empty.Root>
								{:else}
									{@const proposal = activeGroupingProposal}
									{@const payload = activeGroupingPayload}
									<div class="flex flex-wrap items-center gap-2">
										<Badge variant="secondary">{proposal.status}</Badge>
										<span class="text-xs text-muted-foreground">
											{proposal.provider} / {proposal.model} · prompt {proposal.prompt_version}
										</span>
									</div>

									<div
										class="rounded-md border p-4"
										data-testid="study-grouping-choice"
									>
										<p class="text-sm font-medium">Suggested destination</p>
										{#if payload.choice.kind === 'existing_study'}
											<div class="mt-2 flex flex-wrap items-center gap-2">
												<Badge variant="secondary">Existing study</Badge>
												<span class="font-medium"
													>{studyLabel(payload.choice.study_id)}</span
												>
												<span class="text-sm text-muted-foreground">
													study {payload.choice.study_id} · expected revision
													{payload.choice.expected_revision}
												</span>
											</div>
										{:else}
											<div class="mt-2 flex flex-wrap items-center gap-2">
												<Badge variant="secondary">New study</Badge>
												<span class="font-medium"
													>{payload.choice.title}</span
												>
											</div>
										{/if}
										<p class="mt-2 text-xs text-muted-foreground">
											Accepting applies this proposed membership at the
											recorded revisions. The change can be reversed from the
											study membership controls.
										</p>
										<div class="mt-3 grid gap-2 text-sm sm:grid-cols-2">
											<div class="rounded-md bg-muted/40 p-2">
												<span class="font-medium">Current membership</span>
												<p class="text-xs text-muted-foreground">
													{selectedMembership?.study_id
														? `${studyLabel(selectedMembership.study_id)} · revision ${selectedMembership.study_revision}`
														: 'Unassigned'}
												</p>
											</div>
											<div class="rounded-md bg-muted/40 p-2">
												<span class="font-medium"
													>Proposed previous membership</span
												>
												<p class="text-xs text-muted-foreground">
													{payload.expected_previous_study_id
														? `${studyLabel(payload.expected_previous_study_id)} · revision ${payload.expected_previous_study_revision ?? 'not provided'}`
														: 'No previous study'}
												</p>
											</div>
										</div>
									</div>

									<div class="rounded-md border p-4">
										<p class="text-sm font-medium">Rationale</p>
										<p class="mt-1 text-sm text-muted-foreground">
											{payload.rationale}
										</p>
									</div>

									<div
										class="rounded-md border p-4"
										data-testid="study-grouping-provenance"
									>
										<p class="text-sm font-medium">Typed metadata provenance</p>
										{#if payload.provenance.length}
											<ul class="mt-2 flex flex-col gap-2 text-xs">
												{#each payload.provenance as evidence (evidence.kind + evidence.field + evidence.content_hash)}
													<li class="rounded-md bg-muted/40 p-2">
														<div class="flex flex-wrap gap-x-2 gap-y-1">
															<span class="font-medium"
																>{groupingEvidenceSubject(
																	evidence
																)}</span
															>
															<span class="text-muted-foreground"
																>· {groupingFieldLabel(
																	evidence.field
																)}</span
															>
														</div>
														<code
															class="mt-1 block break-all text-muted-foreground"
															>content hash: {evidence.content_hash}</code
														>
													</li>
												{/each}
											</ul>
										{:else}
											<p class="mt-1 text-xs text-muted-foreground">
												No provenance recorded.
											</p>
										{/if}
									</div>

									{#if payload.uncertainties.length}
										<Alert.Root role="status">
											<Alert.Title>Uncertainty noted</Alert.Title>
											<Alert.Description>
												<ul class="list-disc pl-5">
													{#each payload.uncertainties as uncertainty (uncertainty)}
														<li>{uncertainty}</li>
													{/each}
												</ul>
											</Alert.Description>
										</Alert.Root>
									{/if}
								{/if}
							</Card.Content>
							<Card.Footer class="flex flex-wrap justify-end gap-2">
								{#if reportId && !activeGroupingProposal}
									<Button
										variant="outline"
										onclick={() => void generateGrouping()}
										disabled={groupingAction !== null}
									>
										{#if groupingAction === 'generate'}<Spinner
												data-icon="inline-start"
											/>{:else}<Brain data-icon="inline-start" />{/if}
										Suggest study group
									</Button>
								{:else if activeGroupingProposal}
									<Button
										variant="outline"
										disabled={pendingGroupingProposalId !== null}
										onclick={() => void decideGrouping('reject')}
									>
										{#if groupingAction === 'reject'}<Spinner
												data-icon="inline-start"
											/>{:else}<X data-icon="inline-start" />{/if}
										Reject
									</Button>
									<Button
										disabled={pendingGroupingProposalId !== null}
										onclick={() => void decideGrouping('accept')}
									>
										{#if groupingAction === 'accept'}<Spinner
												data-icon="inline-start"
											/>{:else}<Check data-icon="inline-start" />{/if}
										Accept and apply
									</Button>
								{/if}
							</Card.Footer>
						</Card.Root>

						<div>
							<h2 class="text-lg font-semibold">Reports in this investigation</h2>
							<div class="mt-3 flex flex-col gap-2">
								{#each selectedStudy.reports as report (report.report_id)}<div
										class="flex items-center justify-between gap-3 rounded-md border p-3"
									>
										<div>
											<p class="font-medium">
												{report.title ?? report.report_id}
											</p>
											<p class="text-xs text-muted-foreground">
												{report.role} · {report.report_id}
											</p>
										</div>
										<Button
											type="button"
											variant="ghost"
											size="sm"
											onclick={() => void unassignReport(report.report_id)}
											>Unassign</Button
										>
									</div>{:else}<p class="text-sm text-muted-foreground">
										No reports assigned yet.
									</p>{/each}
							</div>
						</div>
					</Card.Content>
				</Card.Root>

				<Card.Root>
					<Card.Header
						><Card.Title>Grouping history</Card.Title><Card.Description
							>Every group, move, unassign, rename, and classification change remains
							visible.</Card.Description
						></Card.Header
					>
					<Card.Content
						><ol class="flex flex-col gap-3" aria-live="polite">
							{#each history as event (event.id)}<li
									class="rounded-md border p-3 text-sm"
								>
									<div class="flex flex-wrap items-center justify-between gap-2">
										<Badge variant="outline">{event.event_type}</Badge><span
											class="text-xs text-muted-foreground"
											>{new Date(event.created_at).toLocaleString()} · {event.actor_id}</span
										>
									</div>
									<p class="mt-2 text-xs text-muted-foreground">
										revision {event.before_revision} → {event.result_revision}{event.report_id
											? ` · report ${event.report_id}`
											: ''}
									</p>
								</li>{:else}<li class="text-sm text-muted-foreground">
									No history yet.
								</li>{/each}
						</ol></Card.Content
					>
				</Card.Root>
			</div>
		{:else}
			<Card.Root
				><Card.Content class="p-10 text-center text-muted-foreground"
					>Select a study group or create one to inspect membership and provenance.</Card.Content
				></Card.Root
			>
		{/if}
	</div>
</main>
