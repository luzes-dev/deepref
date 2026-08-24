<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
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
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import * as Select from '$lib/components/ui/select';
	import { Separator } from '$lib/components/ui/separator';
	import { parseStudyLocation, updateStudyLocation } from '../url';
	import StudyClassificationForm from './StudyClassificationForm.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	let newTitle = $state('');
	let renameTitle = $state('');
	let reportId = $state('');
	let role = $state<StudyReportRole>(StudyReportRoleInput.report_of_study);
	let formError = $state<string | undefined>();

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
	const createMutation = createCreateProjectStudy();
	const renameMutation = createRenameProjectStudy();
	const classifyMutation = createClassifyProjectStudy();
	const membershipMutation = createPutReportStudyMembership();

	const studies = $derived(studiesQuery.data?.data.items ?? []);
	const reports = $derived(reportsQuery.data?.data.items ?? []);
	const selectedStudy = $derived(studyQuery.data?.data);
	const selectedMembership = $derived(
		membershipQuery.data?.data &&
			typeof membershipQuery.data.data === 'object' &&
			'study_id' in membershipQuery.data.data
			? membershipQuery.data.data
			: undefined
	);
	const history = $derived(historyQuery.data?.data ?? []);
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
										>{reportId || 'Choose report'}</Select.Trigger
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
