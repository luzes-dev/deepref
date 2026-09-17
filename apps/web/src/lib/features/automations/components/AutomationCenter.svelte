<script lang="ts">
	import { tick } from 'svelte';
	import {
		createConfigureAutomationDefinition,
		createGetAutomationRun,
		createListAutomationDefinitions,
		createListAutomationRuns,
		getListAutomationDefinitionsQueryKey,
		getListAutomationRunsQueryKey,
		triggerAutomationManually
	} from '$lib/api/generated/automations/automations';
	import type {
		AutomationDefinitionDto,
		ListAutomationRunsParams
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import { useQueryClient } from '@tanstack/svelte-query';
	import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import { StatePanel, Surface } from '@deepref/ui/layout';
	import * as Alert from '@deepref/ui/alert';
	import { notifyInfo } from '$lib/features/notifications/toast';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import AutomationEditor from './AutomationEditor.svelte';
	import RecipeLibrary from './RecipeLibrary.svelte';
	import {
		AUTOMATION_RECIPE_ROUTE,
		DEFAULT_AUTOMATION_DRAFT,
		draftFromDefinition,
		isActiveAutomationRun,
		isProjectMaintenanceDefinition,
		isAutomationStatus,
		isAutomationTrigger,
		labelForStatus,
		labelForTrigger,
		type AutomationDraft
	} from '../helpers';
	import {
		automationGraphStorageKey,
		saveAutomationGraphDraft,
		type AutomationEditorSavePayload
	} from '../graph';
	import type { PredefinedRecipe } from '../recipes';

	let { projectId }: { projectId: string } = $props();

	const RUN_LIST_PARAMS = { limit: 25 } satisfies ListAutomationRunsParams;
	const ACTOR_HEADERS = {
		'x-actor-kind': 'user',
		'x-actor-id': 'local-user'
	} satisfies Record<string, string>;

	const queryClient = useQueryClient();
	const definitionsQuery = createListAutomationDefinitions(() => projectId);
	const runsQuery = createListAutomationRuns(
		() => projectId,
		() => RUN_LIST_PARAMS,
		() => ({
			query: {
				refetchInterval: (query) =>
					query.state.data?.data.some((run) => isActiveAutomationRun(run))
						? 2_000
						: false,
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const configureMutation = createConfigureAutomationDefinition(() => ({
		request: { headers: ACTOR_HEADERS }
	}));

	type CenterView = 'list' | 'editor';
	type EditorMode = 'add' | 'edit';
	type HubView = 'automations' | 'recipes';

	let view = $state<CenterView>('list');
	let hubView = $state<HubView>('automations');
	let editorMode = $state<EditorMode>('add');
	let selectedDefinitionId = $state<string | null>(null);
	let manualPending = $state(false);
	let savePending = $state(false);
	let manualError = $state<unknown>(null);
	let feedback = $state<string | null>(null);
	let validationError = $state<string | null>(null);
	let localDraft = $state<AutomationDraft | null>(null);
	let graphDirty = $state(false);
	let initialDraft = $state<AutomationDraft>({ ...DEFAULT_AUTOMATION_DRAFT });
	let selectedRunId = $state<string | null>(null);
	let runsForceOpen = $state(false);

	type ReturnFocusDescriptor = {
		target: HTMLElement | null;
		testId: string | null;
		runId: string | null;
	};

	// The list branch is destroyed while the native modal is open. Keep the
	// opener's identity so focus can move to the replacement button afterward.
	let returnFocusDescriptor: ReturnFocusDescriptor | null = null;

	const definitions = $derived(definitionsQuery.data?.data ?? []);
	const supportedDefinitions = $derived(
		definitions.filter(isProjectMaintenanceDefinition).slice().sort(compareDefinitions)
	);
	const unsupportedDefinitions = $derived(
		definitions.filter((definition) => !isProjectMaintenanceDefinition(definition))
	);
	const runs = $derived(runsQuery.data?.data ?? []);
	const selectedDefinition = $derived.by(() => {
		if (editorMode !== 'edit' || selectedDefinitionId === null) return undefined;
		return supportedDefinitions.find((definition) => definition.id === selectedDefinitionId);
	});
	const serverDraft = $derived(
		selectedDefinition ? draftFromDefinition(selectedDefinition) : undefined
	);
	const draft = $derived(localDraft ?? serverDraft ?? DEFAULT_AUTOMATION_DRAFT);
	const isDirty = $derived(JSON.stringify(draft) !== JSON.stringify(serverDraft ?? initialDraft));
	const nameAlreadyUsed = $derived(
		editorMode === 'add' &&
			supportedDefinitions.some(
				(definition) =>
					definition.name.trim().toLocaleLowerCase() ===
					draft.name.trim().toLocaleLowerCase()
			)
	);
	const nameIsValid = $derived(
		draft.name.trim().length > 0 && draft.name.trim().length <= 200 && !nameAlreadyUsed
	);
	const selectedRunQuery = createGetAutomationRun(
		() => projectId,
		() => selectedRunId ?? '',
		() => ({
			query: {
				enabled: selectedRunId !== null,
				refetchInterval: (query) =>
					query.state.data?.data && isActiveAutomationRun(query.state.data.data)
						? 2_000
						: false,
				refetchIntervalInBackground: false
			}
		})
	);
	const selectedRun = $derived(
		selectedRunQuery.data?.data ?? runs.find((run) => run.id === selectedRunId)
	);
	const manualDefinitionIsReady = $derived(
		selectedDefinition?.status === 'active' && selectedDefinition.trigger === 'manual'
	);
	const configurationError = $derived(
		configureMutation.error ? configurationErrorMessage(configureMutation.error) : null
	);
	const editorError = $derived(configurationError ?? validationError);
	type AutomationPageState = 'loading' | 'error' | 'empty' | 'ready';
	const pageState = $derived<AutomationPageState>(
		definitionsQuery.error
			? 'error'
			: definitionsQuery.isPending
				? 'loading'
				: supportedDefinitions.length === 0
					? 'empty'
					: 'ready'
	);

	function compareDefinitions(
		left: AutomationDefinitionDto,
		right: AutomationDefinitionDto
	): number {
		return left.name.localeCompare(right.name) || left.id.localeCompare(right.id);
	}

	async function invalidateAutomationQueries(): Promise<void> {
		await Promise.all([
			queryClient.invalidateQueries({
				queryKey: getListAutomationDefinitionsQueryKey(projectId)
			}),
			queryClient.invalidateQueries({
				queryKey: getListAutomationRunsQueryKey(projectId, RUN_LIST_PARAMS)
			})
		]);
	}

	async function refreshAutomationQueries(): Promise<void> {
		await invalidateAutomationQueries();
		await Promise.all([definitionsQuery.refetch(), runsQuery.refetch()]);
		if (selectedRunId) await selectedRunQuery.refetch();
	}

	async function retryQueries(): Promise<void> {
		await Promise.all([definitionsQuery.refetch(), runsQuery.refetch()]);
	}

	function openDefinitionEditor(definitionId: string, event?: MouseEvent): void {
		if (savePending) return;
		const definition = supportedDefinitions.find((candidate) => candidate.id === definitionId);
		if (!definition) return;
		captureEditorOpener(event?.currentTarget);
		view = 'editor';
		editorMode = 'edit';
		selectedDefinitionId = definition.id;
		manualError = null;
		validationError = null;
		configureMutation.reset();
		localDraft = null;
		graphDirty = false;
		selectedRunId = null;
	}

	function startAddingDefinition(event?: MouseEvent, baseName?: string): void {
		if (savePending) return;
		captureEditorOpener(event?.currentTarget);
		view = 'editor';
		editorMode = 'add';
		selectedDefinitionId = null;
		manualError = null;
		validationError = null;
		configureMutation.reset();
		let name = baseName ?? 'New automation';
		let suffix = 2;
		while (
			supportedDefinitions.some(
				(definition) => definition.name.toLocaleLowerCase() === name.toLocaleLowerCase()
			)
		) {
			name = `${baseName ?? 'New automation'} ${suffix++}`;
		}
		localDraft = { ...DEFAULT_AUTOMATION_DRAFT, name };
		initialDraft = { ...localDraft };
		graphDirty = false;
		selectedRunId = null;
	}

	function forkRecipe(recipe: PredefinedRecipe): void {
		startAddingDefinition(undefined, recipe.title);
	}

	function captureEditorOpener(eventTarget?: EventTarget | null): void {
		const target =
			eventTarget instanceof HTMLElement
				? eventTarget
				: typeof document !== 'undefined' && document.activeElement instanceof HTMLElement
					? document.activeElement
					: null;
		returnFocusDescriptor = {
			target,
			testId: target?.dataset.testid ?? null,
			runId: target?.dataset.automationRunId ?? null
		};
	}

	function restoreEditorFocus(): void {
		const descriptor = returnFocusDescriptor;
		returnFocusDescriptor = null;
		if (!descriptor) return;

		void tick()
			.then(() => {
				const restore = () => {
					let target = descriptor.target?.isConnected ? descriptor.target : null;
					if (!target && descriptor.testId) {
						target =
							Array.from(
								document.querySelectorAll<HTMLElement>('[data-testid]')
							).find(
								(candidate) =>
									candidate.dataset.testid === descriptor.testId &&
									(descriptor.runId === null ||
										candidate.dataset.automationRunId === descriptor.runId)
							) ?? null;
					}
					target?.focus({ preventScroll: true });
				};

				if (typeof window.requestAnimationFrame === 'function') {
					window.requestAnimationFrame(restore);
				} else {
					restore();
				}
			})
			.catch(() => undefined);
	}

	async function saveConfiguration(payload?: AutomationEditorSavePayload): Promise<boolean> {
		const nextDraft = payload?.draft ?? draft;
		const normalizedName = nextDraft.name.trim();
		const nameAlreadyUsed = supportedDefinitions.some(
			(definition) =>
				editorMode === 'add' &&
				definition.name.trim().toLocaleLowerCase() === normalizedName.toLocaleLowerCase()
		);
		if (
			normalizedName.length === 0 ||
			normalizedName.length > 200 ||
			!isAutomationTrigger(nextDraft.trigger) ||
			!isAutomationStatus(nextDraft.status)
		) {
			validationError = 'Name is required and must be at most 200 characters.';
			return false;
		}
		if (nameAlreadyUsed) {
			validationError = 'A definition with this name already exists.';
			return false;
		}
		if (
			savePending ||
			configureMutation.isPending ||
			(editorMode === 'edit' && !selectedDefinition)
		)
			return false;

		const addingDefinition = editorMode === 'add';
		savePending = true;
		manualError = null;
		validationError = null;
		try {
			const response = await configureMutation.mutateAsync({
				projectId,
				recipe: AUTOMATION_RECIPE_ROUTE,
				data: {
					name: normalizedName,
					trigger: nextDraft.trigger,
					status: nextDraft.status
				}
			});
			await refreshAutomationQueries();
			if (payload) {
				graphDirty = !saveAutomationGraphDraft(
					automationGraphStorageKey(projectId, response.data.id),
					payload.graph
				);
			}
			localDraft = null;
			editorMode = 'edit';
			selectedDefinitionId = response.data.id;
			feedback = addingDefinition ? 'Automation created.' : 'Automation settings saved.';
			if (graphDirty) feedback += ' The graph could not be saved in this browser.';
			notifyInfo('Automations', feedback);
			return !graphDirty;
		} catch {
			// The mutation error is rendered in the editor.
			return false;
		} finally {
			savePending = false;
		}
	}

	function closeEditor(): void {
		if (savePending) return;
		if (
			(isDirty || graphDirty) &&
			typeof window !== 'undefined' &&
			!window.confirm('Discard your unsaved automation changes?')
		)
			return;

		view = 'list';
		editorMode = 'add';
		selectedDefinitionId = null;
		manualError = null;
		validationError = null;
		configureMutation.reset();
		localDraft = null;
		graphDirty = false;
		selectedRunId = null;
	}

	async function runManually(): Promise<void> {
		const definition = selectedDefinition;
		if (!definition || !manualDefinitionIsReady || manualPending) return;

		manualPending = true;
		manualError = null;
		try {
			const response = await triggerAutomationManually(
				projectId,
				{ definition_id: definition.id },
				{
					headers: {
						...ACTOR_HEADERS,
						'Idempotency-Key': crypto.randomUUID()
					}
				}
			);
			selectedRunId = response.data.run_id;
			await refreshAutomationQueries();
			feedback = response.data.created
				? 'Automation run queued.'
				: 'That automation run was already queued; the existing run is shown below.';
			notifyInfo('Automations', feedback);
		} catch (error: unknown) {
			manualError = error;
		} finally {
			manualPending = false;
		}
	}

	async function handleRunQueued(runId: string): Promise<void> {
		hubView = 'automations';
		await refreshAutomationQueries();
		selectedRunId = runId;
		runsForceOpen = true;
	}

	function queryErrorMessage(error: unknown): string {
		return error instanceof Error ? error.message : 'The automation data could not be loaded.';
	}

	function configurationErrorMessage(error: unknown): string {
		if (error instanceof ApiError && error.status === 409) {
			return 'The recipe configuration changed while you were editing. Refresh and try again.';
		}
		return queryErrorMessage(error);
	}

	function manualErrorMessage(error: unknown): string {
		if (
			error instanceof ApiError &&
			error.status === 409 &&
			error.code === 'AUTOMATION_PAUSED'
		) {
			return 'This automation is paused. Set the definition to active before running it manually.';
		}
		if (error instanceof ApiError && error.status === 409) {
			return 'The automation could not start because its definition changed. Refresh and try again.';
		}
		return queryErrorMessage(error);
	}

	function badgeVariant(status: string): 'default' | 'secondary' | 'destructive' | 'outline' {
		if (status === 'failed' || status === 'dead') return 'destructive';
		if (status === 'completed' || status === 'active') return 'secondary';
		return 'outline';
	}
</script>

<div
	class="relative h-full min-h-0 w-full overflow-hidden bg-background"
	data-testid="automation-page"
	data-automation-state={pageState}
	data-automation-view={view}
>
	{#if view === 'editor'}
		<AutomationEditor
			{projectId}
			mode={editorMode}
			definition={selectedDefinition}
			{draft}
			isSaving={savePending}
			{isDirty}
			{nameIsValid}
			{nameAlreadyUsed}
			configurationError={editorError}
			{feedback}
			manualReady={manualDefinitionIsReady}
			{manualPending}
			manualError={manualError ? manualErrorMessage(manualError) : null}
			runs={runs.filter((run) => run.definition_id === selectedDefinitionId)}
			{selectedRun}
			selectedRunLoading={selectedRunId !== null && selectedRunQuery.isPending}
			selectedRunError={selectedRunQuery.error?.message ?? null}
			onDraftChange={(next) => {
				if (!savePending) localDraft = next;
			}}
			onDirtyChange={(dirty) => {
				graphDirty = dirty;
			}}
			onSave={saveConfiguration}
			onClose={closeEditor}
			onReturnFocus={restoreEditorFocus}
			onRunManually={runManually}
			onRefresh={refreshAutomationQueries}
			onSelectRun={(id) => {
				selectedRunId = id;
			}}
		/>
	{:else}
		<div class="mx-auto flex h-full min-h-0 w-full max-w-4xl flex-col overflow-auto">
			<header
				class="flex flex-wrap items-start justify-between gap-4 border-b border-border/70 px-5 py-6 sm:px-8 sm:py-8"
			>
				<div class="min-w-0">
					<p class="text-xs font-semibold tracking-[0.18em] text-primary uppercase">
						Evidence operations
					</p>
					<h1
						class="editorial-title mt-2 text-3xl leading-tight text-foreground sm:text-4xl"
					>
						Automations
					</h1>
					<p class="mt-2 max-w-xl text-sm leading-relaxed text-muted-foreground">
						Run predefined evidence recipes or keep custom project maintenance
						automations visible, predictable, and ready to run.
					</p>
				</div>
				<div class="flex shrink-0 items-center gap-2">
					<Button
						variant="ghost"
						size="icon"
						aria-label="Refresh automations"
						disabled={definitionsQuery.isFetching || runsQuery.isFetching}
						onclick={() => void refreshAutomationQueries()}
						data-testid="automation-refresh"
					>
						{#if definitionsQuery.isFetching || runsQuery.isFetching}<Spinner
							/>{:else}<RefreshCwIcon aria-hidden="true" />{/if}
					</Button>
					<Button onclick={startAddingDefinition} data-testid="automation-add-definition">
						<PlusIcon data-icon="inline-start" aria-hidden="true" />
						New automation
					</Button>
				</div>
			</header>

			<div class="flex min-h-0 flex-1 flex-col gap-6 px-5 py-6 sm:px-8 sm:py-8">
				<div
					class="flex w-fit items-center gap-1 rounded-full border border-border/70 bg-muted/40 p-1"
					role="group"
					aria-label="Automations hub view"
					data-testid="hub-view-toggle"
				>
					<button
						type="button"
						class="rounded-full px-4 py-1.5 text-sm font-medium transition-colors {hubView ===
						'automations'
							? 'bg-background text-foreground shadow-2xs'
							: 'text-muted-foreground hover:text-foreground'}"
						aria-pressed={hubView === 'automations'}
						onclick={() => (hubView = 'automations')}
						data-testid="hub-view-automations"
					>
						Automations
					</button>
					<button
						type="button"
						class="rounded-full px-4 py-1.5 text-sm font-medium transition-colors {hubView ===
						'recipes'
							? 'bg-background text-foreground shadow-2xs'
							: 'text-muted-foreground hover:text-foreground'}"
						aria-pressed={hubView === 'recipes'}
						onclick={() => (hubView = 'recipes')}
						data-testid="hub-view-recipes"
					>
						Recipe Library
					</button>
				</div>

				{#if hubView === 'recipes'}
					<RecipeLibrary
						{projectId}
						{definitions}
						onFork={forkRecipe}
						onDefinitionCreated={refreshAutomationQueries}
						onRunQueued={handleRunQueued}
					/>
				{:else}
					<div
						class="flex min-h-0 flex-1 flex-col gap-6"
						data-testid="automation-manager"
					>
						{#if definitionsQuery.error}
							<Alert.Root
								variant="destructive"
								data-testid="automation-query-error"
								role="alert"
							>
								<AlertCircleIcon />
								<Alert.Title>Automations unavailable</Alert.Title>
								<Alert.Description
									>{queryErrorMessage(definitionsQuery.error)}</Alert.Description
								>
								<Button
									variant="outline"
									size="sm"
									onclick={() => void retryQueries()}>Retry</Button
								>
							</Alert.Root>
						{:else if definitionsQuery.isPending}
							<div data-testid="automation-list-loading">
								<StatePanel
									state="loading"
									title="Loading automations"
									description="Checking the automations configured for this project."
								/>
							</div>
						{:else if supportedDefinitions.length === 0}
							<div data-testid="automation-definitions-empty">
								<div
									data-testid="automation-list-empty"
									data-automation-empty="true"
								>
									<StatePanel
										state="empty"
										title="No automations yet"
										description="Create a project maintenance automation to make repeatable work easier to follow."
									>
										{#snippet action()}
											<Button
												onclick={startAddingDefinition}
												data-testid="automation-empty-create"
											>
												<PlusIcon
													data-icon="inline-start"
													aria-hidden="true"
												/>
												Create automation
											</Button>
										{/snippet}
									</StatePanel>
								</div>
							</div>
						{:else}
							<section
								class="flex flex-col gap-3"
								aria-labelledby="automation-list-heading"
							>
								<div class="flex items-end justify-between gap-3">
									<div>
										<h2
											id="automation-list-heading"
											class="text-base font-semibold text-foreground"
										>
											Your automations
										</h2>
										<p class="mt-1 text-sm text-muted-foreground">
											{supportedDefinitions.length} configured
											{supportedDefinitions.length === 1
												? 'automation'
												: 'automations'}
										</p>
									</div>
									{#if runs.some((run) => isActiveAutomationRun(run))}
										<Badge variant="outline">Activity in progress</Badge>
									{/if}
								</div>

								<div
									class="flex flex-col divide-y divide-border/70 rounded-xl border border-border/80 bg-card"
								>
									{#each supportedDefinitions as definition (definition.id)}
										<article
											class="group flex flex-wrap items-center gap-4 px-4 py-4 transition-colors first:rounded-t-xl last:rounded-b-xl hover:bg-muted/25 sm:px-5"
											data-testid={`automation-definition-card-${definition.id}`}
											data-automation-id={definition.id}
										>
											<div class="min-w-0 flex-1">
												<div
													class="flex min-w-0 flex-wrap items-center gap-2"
												>
													<h3
														class="truncate font-medium text-foreground"
													>
														{definition.name}
													</h3>
													<Badge
														variant={badgeVariant(definition.status)}
													>
														{labelForStatus(definition.status)}
													</Badge>
												</div>
												<p class="mt-1 text-sm text-muted-foreground">
													Runs on {labelForTrigger(definition.trigger)}
												</p>
											</div>
											<div class="flex items-center gap-2">
												<span
													class="hidden text-xs text-muted-foreground sm:inline"
												>
													{definition.steps.length}
													{definition.steps.length === 1
														? 'step'
														: 'steps'}
												</span>
												<Button
													variant="ghost"
													size="sm"
													onclick={(event) =>
														openDefinitionEditor(definition.id, event)}
													data-testid={`automation-edit-definition-${definition.id}`}
												>
													<PencilIcon
														data-icon="inline-start"
														aria-hidden="true"
													/>
													Edit
													<ChevronRightIcon
														data-icon="inline-end"
														aria-hidden="true"
													/>
												</Button>
											</div>
										</article>
									{/each}
								</div>
							</section>
						{/if}

						{#if unsupportedDefinitions.length > 0}
							<Alert.Root data-testid="automation-unsupported-definitions">
								<AlertCircleIcon />
								<Alert.Title>Some automations are unavailable here</Alert.Title>
								<Alert.Description>
									These automations use recipes this editor does not support: {unsupportedDefinitions
										.map((definition) => definition.name)
										.join(', ')}.
								</Alert.Description>
							</Alert.Root>
						{/if}

						<details
							class="group"
							open={runsQuery.isPending || runsForceOpen}
							data-testid="automation-run-history"
						>
							<summary
								class="flex cursor-pointer list-none items-center justify-between gap-3 py-1 text-sm text-muted-foreground marker:hidden"
							>
								<span class="font-medium text-foreground">Recent activity</span>
								<span class="text-xs group-open:hidden">{runs.length} runs</span>
							</summary>
							<div class="mt-3 flex flex-col gap-2">
								{#if runsQuery.error}
									<Alert.Root
										variant="destructive"
										data-testid="automation-runs-error"
									>
										<AlertCircleIcon />
										<Alert.Title>Recent activity unavailable</Alert.Title>
										<Alert.Description
											>{queryErrorMessage(runsQuery.error)}</Alert.Description
										>
										<Button
											variant="outline"
											size="sm"
											onclick={() => void runsQuery.refetch()}>Retry</Button
										>
									</Alert.Root>
								{:else if runsQuery.isPending}
									<div data-testid="automation-runs-loading">
										<Surface tone="subtle" class="p-4">
											<div
												class="flex items-center gap-2 text-sm text-muted-foreground"
											>
												<Spinner /> Loading recent activity
											</div>
										</Surface>
									</div>
								{:else if runs.length === 0}
									<div data-testid="automation-runs-empty">
										<Surface tone="subtle" class="p-4">
											<p class="text-sm text-muted-foreground">
												No automation runs yet.
											</p>
										</Surface>
									</div>
								{:else}
									<div
										class="flex flex-col divide-y divide-border/70 rounded-lg border border-border/80 bg-card"
										data-testid="automation-runs"
									>
										{#each runs.slice(0, 5) as run (run.id)}
											<button
												type="button"
												class="flex items-center justify-between gap-3 px-4 py-3 text-left text-sm hover:bg-muted/25"
												data-testid="automation-run"
												data-automation-run-id={run.id}
												disabled={!supportedDefinitions.some(
													(definition) =>
														definition.id === run.definition_id
												)}
												onclick={(event) => {
													openDefinitionEditor(run.definition_id, event);
													selectedRunId = run.id;
												}}
											>
												<span class="min-w-0 truncate text-muted-foreground"
													>Run {run.id.slice(0, 8)}</span
												>
												<Badge variant={badgeVariant(run.status)}
													>{labelForStatus(run.status)}</Badge
												>
											</button>
										{/each}
									</div>
								{/if}
							</div>
						</details>
					</div>
				{/if}
			</div>
		</div>
	{/if}
</div>
