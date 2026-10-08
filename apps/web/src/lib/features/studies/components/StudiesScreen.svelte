<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		createDecideAiProposal,
		createGenerateStudyGroupingSuggestion,
		createListAiProposals,
		getListAiProposalsQueryKey
	} from '$lib/api/generated/ai/ai';
	import { ApiError } from '$lib/api/custom-fetch';
	import type {
		AiProposalDecisionInput,
		AiProposalDto,
		AiStudyGroupingProposalPayload
	} from '$lib/api/generated/models';
	import {
		StudyReportRoleInput,
		type StudyReportRoleInput as StudyReportRole
	} from '$lib/api/generated/models/studyReportRoleInput';
	import { createListFullTextScreeningQueue } from '$lib/api/generated/documents/documents';
	import { createListProjectReports } from '$lib/api/generated/reports/reports';
	import type { UngroupedReportDto } from '$lib/api/generated/models';
	import {
		createClassifyProjectStudy,
		createCreateProjectStudy,
		createGetProjectStudy,
		createGetReportStudyMembership,
		createListProjectStudies,
		createListProjectStudyHistory,
		createListUngroupedIncludedReports,
		createPutReportStudyMembership,
		createRenameProjectStudy,
		getGetProjectStudyQueryKey,
		getGetReportStudyMembershipQueryKey,
		getListProjectStudiesQueryKey,
		getListUngroupedIncludedReportsQueryKey,
		getListProjectStudyHistoryQueryKey
	} from '$lib/api/generated/studies/studies';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as Tabs from '@deepref/ui/tabs';
	import * as Resizable from '@deepref/ui/resizable';
	import { MediaQuery } from 'svelte/reactivity';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ReviewRunObserver } from '$lib/features/ai-assistance/review-run-observer.svelte';
	import { notifyError, notifyWarning } from '$lib/features/notifications/toast';
	import { parseStudyLocation, updateStudyLocation } from '../url';
	import StudyClassificationAssistance from './StudyClassificationAssistance.svelte';
	import StudyDetailsPanel from './StudyDetailsPanel.svelte';
	import StudyGroupingAssistance from './StudyGroupingAssistance.svelte';
	import StudyHistoryPanel from './StudyHistoryPanel.svelte';
	import StudyListPanel from './StudyListPanel.svelte';
	import StudyMembershipPanel from './StudyMembershipPanel.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	const wide = new MediaQuery('(min-width: 1024px)');
	const location = $derived(parseStudyLocation(page.url.searchParams));
	const selectedStudyId = $derived(location.studyId);
	let newTitle = $state('');
	let renameTitle = $state('');
	let reportId = $derived(location.reportId ?? '');
	let role = $state<StudyReportRole>(StudyReportRoleInput.report_of_study);
	let groupingAction = $state<'generate' | 'accept' | 'reject' | null>(null);
	let pendingGroupingProposalId = $state<string | null>(null);
	let classificationAction = $state<'accept' | 'reject' | null>(null);
	let pendingClassificationProposalId = $state<string | null>(null);

	const studiesQuery = createListProjectStudies(
		() => projectId,
		() => ({ limit: 100 })
	);
	const ungroupedQuery = createListUngroupedIncludedReports(() => projectId);
	const reportsQuery = createListProjectReports(
		() => projectId,
		() => ({ limit: 100 })
	);
	const fullTextQuery = createListFullTextScreeningQueue(
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
	const classificationProposalsQuery = createListAiProposals(
		() => projectId,
		() => ({
			status: 'pending',
			task_kind: 'study_design_classification',
			target_study_id: selectedStudyId,
			limit: 1
		}),
		() => ({ query: { enabled: Boolean(selectedStudyId) } })
	);
	const createMutation = createCreateProjectStudy();
	const renameMutation = createRenameProjectStudy();
	const classifyMutation = createClassifyProjectStudy();
	const membershipMutation = createPutReportStudyMembership();
	const generateGroupingMutation = createGenerateStudyGroupingSuggestion();
	const decideGroupingMutation = createDecideAiProposal();
	const decideClassificationMutation = createDecideAiProposal();
	const groupingReviewRun = new ReviewRunObserver(
		() => projectId,
		async () => {
			await groupingProposalsQuery.refetch();
		}
	);

	const studies = $derived(studiesQuery.data?.data.items ?? []);
	const reports = $derived(reportsQuery.data?.data.items ?? []);
	const ungrouped = $derived(ungroupedQuery.data?.data ?? []);
	const includedReportIds = $derived(
		fullTextQuery.data
			? new Set(
					fullTextQuery.data.data.items
						.filter((item) => item.full_text_status === 'include')
						.map((item) => item.report_id)
				)
			: undefined
	);
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
	const groupingErrorMessage = $derived(groupingQueryError || groupingMutationError);
	const groupingConflict = $derived(
		isApiErrorWithStatus(groupingProposalsQuery.error, 409) ||
			isApiErrorWithStatus(generateGroupingMutation.error, 409) ||
			isApiErrorWithStatus(decideGroupingMutation.error, 409)
	);
	const groupingProviderUnavailable = $derived(
		isApiErrorWithStatus(groupingProposalsQuery.error, 503) ||
			isApiErrorWithStatus(generateGroupingMutation.error, 503) ||
			isApiErrorWithStatus(decideGroupingMutation.error, 503)
	);
	const classificationProposals = $derived(
		(classificationProposalsQuery.data?.data.items ?? []).filter((proposal) =>
			isClassificationProposal(proposal, selectedStudyId)
		)
	);
	const activeClassificationProposal = $derived(classificationProposals[0]);
	const classificationQueryError = $derived(classificationProposalsQuery.error?.message ?? '');
	const classificationMutationError = $derived(decideClassificationMutation.error?.message ?? '');
	const classificationErrorMessage = $derived(
		classificationQueryError || classificationMutationError
	);
	const classificationConflict = $derived(
		isApiErrorWithStatus(classificationProposalsQuery.error, 409) ||
			isApiErrorWithStatus(decideClassificationMutation.error, 409)
	);
	const classificationProviderUnavailable = $derived(
		isApiErrorWithStatus(classificationProposalsQuery.error, 503) ||
			isApiErrorWithStatus(classificationProposalsQuery.error, 503) ||
			isApiErrorWithStatus(decideClassificationMutation.error, 503)
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

	type ClassificationProposal = AiProposalDto & {
		payload: Extract<AiProposalDto['payload'], { kind: 'classification' }>;
	};

	function isClassificationProposal(
		proposal: AiProposalDto,
		studyId: string | undefined
	): proposal is ClassificationProposal {
		return (
			Boolean(studyId) &&
			proposal.status === 'pending' &&
			proposal.task_kind === 'study_design_classification' &&
			proposal.target_study_id === studyId &&
			proposal.payload.kind === 'classification' &&
			proposal.payload.study_id === studyId
		);
	}

	function groupingPayload(proposal: AiProposalDto): AiStudyGroupingProposalPayload | undefined {
		switch (proposal.payload.kind) {
			case 'study_grouping':
				return proposal.payload;
			case 'screening':
			case 'duplicate':
			case 'classification':
			case 'appraisal_prefill':
			case 'data_extraction':
				return undefined;
			default: {
				const exhaustive: never = proposal.payload;
				return exhaustive;
			}
		}
	}

	function studyLabel(studyId: string): string {
		return studies.find((study) => study.id === studyId)?.title ?? studyId;
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
		if (!reportId || groupingAction || groupingReviewRun.isActive) return;
		groupingAction = 'generate';
		try {
			const response = await generateGroupingMutation.mutateAsync({ projectId, reportId });
			await groupingReviewRun.observe(response.data);
		} catch (error) {
			notifyError('Study grouping suggestion failed', error);
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
			if (error instanceof ApiError && error.status === 409) {
				notifyWarning(
					'Study grouping changed elsewhere',
					'The proposal remains pending; refresh and review it again.'
				);
			} else {
				notifyError('The study grouping decision failed', error);
			}
			await groupingProposalsQuery.refetch();
		} finally {
			pendingGroupingProposalId = null;
			groupingAction = null;
		}
	}

	async function refreshClassificationQueries(): Promise<void> {
		await queryClient.invalidateQueries({
			queryKey: getListAiProposalsQueryKey(projectId)
		});
		await refreshStudy();
		await classificationProposalsQuery.refetch();
	}

	async function decideClassification(decision: AiProposalDecisionInput): Promise<void> {
		if (!activeClassificationProposal || pendingClassificationProposalId) return;
		const proposal = activeClassificationProposal;
		pendingClassificationProposalId = proposal.id;
		classificationAction = decision;
		try {
			await decideClassificationMutation.mutateAsync({
				projectId,
				proposalId: proposal.id,
				data: {
					decision,
					reason: `Human reviewer ${decision === 'accept' ? 'accepted' : 'rejected'} study design classification suggestion.`
				}
			});
			await refreshClassificationQueries();
		} catch (error) {
			if (error instanceof ApiError && error.status === 409) {
				notifyWarning(
					'Study classification changed elsewhere',
					'Refresh the proposal and review it again.'
				);
			} else if (error instanceof ApiError && error.status === 503) {
				notifyWarning(
					'AI provider unavailable',
					'The proposal remains unchanged; try again later.'
				);
			} else {
				notifyError('The study classification decision failed', error);
			}
			await classificationProposalsQuery.refetch();
		} finally {
			pendingClassificationProposalId = null;
			classificationAction = null;
		}
	}

	async function selectStudy(studyId: string, replaceState = false): Promise<void> {
		const search = updateStudyLocation(page.url.searchParams, { studyId, reportId: '' });
		let href: string = resolve('/projects/[projectId]/studies', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true, replaceState });
	}

	// Opening Studies always shows a study; an empty detail pane only asks for a click.
	$effect(() => {
		const first = studies[0];
		if (!selectedStudyId && first) void selectStudy(first.id, true);
	});

	async function createStudy(): Promise<void> {
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
			notifyError('Study could not be created', error);
		}
	}

	let groupingReportId = $state<string | null>(null);

	async function createStudyFromReport(report: UngroupedReportDto): Promise<void> {
		groupingReportId = report.report_id;
		try {
			const created = await createMutation.mutateAsync({
				projectId,
				data: { title: (report.title?.trim() || 'Untitled study').slice(0, 200) }
			});
			await membershipMutation.mutateAsync({
				projectId,
				reportId: report.report_id,
				data: {
					study_id: created.data.id,
					role: StudyReportRoleInput.report_of_study,
					expected_revision: created.data.revision
				}
			});
			await Promise.all([
				queryClient.invalidateQueries({
					queryKey: getListProjectStudiesQueryKey(projectId)
				}),
				queryClient.invalidateQueries({
					queryKey: getListUngroupedIncludedReportsQueryKey(projectId)
				})
			]);
			await selectStudy(created.data.id);
		} catch (error) {
			notifyError('Report could not be grouped', error);
		} finally {
			groupingReportId = null;
		}
	}

	async function renameStudy(): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
		try {
			await renameMutation.mutateAsync({
				projectId,
				studyId: selectedStudyId,
				data: { title: renameTitle, expected_revision: selectedStudy.revision }
			});
			renameTitle = '';
			await refreshStudy();
		} catch (error) {
			notifyError('Study could not be renamed', error);
		}
	}

	async function classify(request: {
		design: string;
		physiotherapy: boolean;
		exposure: boolean;
		prediction_or_ai: boolean;
	}): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
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
			notifyError('Study classification failed', error);
		}
	}

	async function assignReport(): Promise<void> {
		if (!selectedStudyId || !selectedStudy || !reportId) return;
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
			notifyError('Report could not be assigned', error);
		}
	}

	async function unassignReport(selectedReportId: string): Promise<void> {
		if (!selectedStudyId || !selectedStudy) return;
		try {
			await membershipMutation.mutateAsync({
				projectId,
				reportId: selectedReportId,
				data: { study_id: null, expected_revision: selectedStudy.revision }
			});
			await refreshStudy(undefined, selectedReportId);
		} catch (error) {
			notifyError('Report could not be unassigned', error);
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

{#snippet detail()}
	{#if selectedStudy}
		<StudyDetailsPanel
			study={selectedStudy}
			{designs}
			bind:renameTitle
			renaming={renameMutation.isPending}
			classifying={classifyMutation.isPending}
			onRename={() => void renameStudy()}
			onClassify={classify}
		>
			<Tabs.Root value="reports">
				<Tabs.List variant="line" aria-label="Study workspace views">
					<Tabs.Trigger value="reports"
						>Reports <span class="text-muted-foreground tabular-nums"
							>{selectedStudy.reports.length}</span
						></Tabs.Trigger
					>
					<Tabs.Trigger value="assistance">AI assistance</Tabs.Trigger>
					<Tabs.Trigger value="history">History</Tabs.Trigger>
				</Tabs.List>

				<Tabs.Content value="reports" class="pt-2">
					<StudyMembershipPanel
						study={selectedStudy}
						{reports}
						{selectedReport}
						{includedReportIds}
						bind:reportId
						bind:role
						assigning={membershipMutation.isPending}
						membershipPending={membershipQuery.isPending || membershipQuery.isFetching}
						currentStudyLabel={selectedMembership &&
						selectedMembership.study_id !== selectedStudy.id
							? studyLabel(selectedMembership.study_id)
							: undefined}
						onAssign={() => void assignReport()}
						onUnassign={(selectedReportId) => void unassignReport(selectedReportId)}
					/>
				</Tabs.Content>
				<Tabs.Content value="assistance" class="flex flex-col gap-6 pt-2">
					<StudyClassificationAssistance
						proposal={activeClassificationProposal}
						pending={classificationProposalsQuery.isPending}
						errorMessage={classificationErrorMessage}
						conflict={classificationConflict}
						providerUnavailable={classificationProviderUnavailable}
						action={classificationAction}
						decisionPending={pendingClassificationProposalId !== null}
						{studyLabel}
						onDecide={(decision) => void decideClassification(decision)}
					/>
					<StudyGroupingAssistance
						{reportId}
						proposal={activeGroupingProposal}
						payload={activeGroupingPayload}
						membership={selectedMembership}
						pending={groupingProposalsQuery.isPending}
						errorMessage={groupingErrorMessage}
						conflict={groupingConflict}
						providerUnavailable={groupingProviderUnavailable}
						action={groupingAction ?? (groupingReviewRun.isActive ? 'generate' : null)}
						decisionPending={pendingGroupingProposalId !== null}
						{studyLabel}
						onGenerate={() => void generateGrouping()}
						onDecide={(decision) => void decideGrouping(decision)}
					/>
				</Tabs.Content>
				<Tabs.Content value="history" class="pt-2"
					><StudyHistoryPanel {history} /></Tabs.Content
				>
			</Tabs.Root>
		</StudyDetailsPanel>
	{:else if selectedStudyId && studyQuery.isPending}
		<div class="flex flex-col gap-3" aria-label="Loading study">
			<Skeleton class="h-7 w-1/2" /><Skeleton class="h-4 w-1/3" /><Skeleton
				class="h-40 w-full"
			/>
		</div>
	{:else if !studiesQuery.isPending && studies.length === 0}
		<div class="flex max-w-md flex-col gap-2">
			<h2 class="editorial-title text-xl">Group included reports into studies</h2>
			<p class="text-sm text-muted-foreground">
				Several reports often describe one investigation. Create a study, then add its
				reports so appraisal and extraction count it once.
			</p>
		</div>
	{/if}
{/snippet}

{#if wide.current}
	<div class="h-full min-h-0" data-testid="studies-page">
		<Resizable.PaneGroup
			direction="horizontal"
			class="h-full"
			autoSaveId="deepref:studies-layout"
		>
			<Resizable.Pane order={1} defaultSize={26} minSize={16} maxSize={40}>
				<StudyListPanel
					{studies}
					{selectedStudyId}
					pending={studiesQuery.isPending}
					creating={createMutation.isPending}
					bind:title={newTitle}
					onCreate={() => void createStudy()}
					onSelect={(studyId) => void selectStudy(studyId)}
					{ungrouped}
					ungroupedPending={ungroupedQuery.isPending}
					{groupingReportId}
					onCreateFromReport={(report) => void createStudyFromReport(report)}
				/>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={2} defaultSize={74} minSize={40}>
				<div class="h-full overflow-y-auto px-8 py-6">
					<div class="max-w-4xl">{@render detail()}</div>
				</div>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
{:else}
	<PageTemplate testId="studies-page" containerClass="gap-6">
		<div class="-mx-4 border-b">
			<StudyListPanel
				{studies}
				{selectedStudyId}
				pending={studiesQuery.isPending}
				creating={createMutation.isPending}
				bind:title={newTitle}
				onCreate={() => void createStudy()}
				onSelect={(studyId) => void selectStudy(studyId)}
				{ungrouped}
				ungroupedPending={ungroupedQuery.isPending}
				{groupingReportId}
				onCreateFromReport={(report) => void createStudyFromReport(report)}
			/>
		</div>
		{@render detail()}
	</PageTemplate>
{/if}
