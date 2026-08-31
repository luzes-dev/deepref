<script lang="ts">
	import {
		createConfigureAutomationDefinition,
		createGetAutomationRun,
		createListAutomationDefinitions,
		createListAutomationRuns,
		getGetAutomationRunQueryKey,
		getListAutomationDefinitionsQueryKey,
		getListAutomationRunsQueryKey,
		triggerAutomationManually
	} from '$lib/api/generated/automations/automations';
	import type {
		AutomationDefinitionDto,
		AutomationRunDto,
		AutomationStepDto,
		ListAutomationRunsParams
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import { useQueryClient } from '@tanstack/svelte-query';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import PlayIcon from '@lucide/svelte/icons/play';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SaveIcon from '@lucide/svelte/icons/save';
	import Settings2Icon from '@lucide/svelte/icons/settings-2';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import * as Select from '$lib/components/ui/select';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import {
		AUTOMATION_RECIPE_ID,
		AUTOMATION_RECIPE_ROUTE,
		AUTOMATION_RECIPE_VERSION,
		AUTOMATION_STATUSES,
		AUTOMATION_TRIGGERS,
		DEFAULT_AUTOMATION_DRAFT,
		draftFromDefinition,
		formatCostMicros,
		formatInteger,
		formatTimestamp,
		isActiveAutomationRun,
		isProjectMaintenanceDefinition,
		isAutomationStatus,
		isAutomationTrigger,
		labelForStatus,
		labelForTrigger,
		type AutomationDraft
	} from '../helpers';

	let { projectId }: { projectId: string } = $props();

	const RUN_LIST_PARAMS = { limit: 25 } satisfies ListAutomationRunsParams;
	const ACTOR_HEADERS = {
		'x-actor-kind': 'user',
		'x-actor-id': 'local-user'
	} satisfies Record<string, string>;
	const BUILT_IN_STEPS = [
		{ ordinal: 0, key: 'recompute_project_metrics', kind: 'deterministic_action' }
	] satisfies AutomationStepDto[];

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

	let selectedRunId = $state<string | null>(null);
	let editorMode = $state<'add' | 'edit'>('add');
	let selectedDefinitionId = $state<string | null>(null);
	let localDraft = $state<AutomationDraft | null>(null);
	let manualPending = $state(false);
	let feedback = $state<string | null>(null);
	let manualError = $state<unknown>(null);

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
	const trimmedDraftName = $derived(draft.name.trim());
	const nameAlreadyUsed = $derived(
		editorMode === 'add' &&
			supportedDefinitions.some(
				(definition) =>
					definition.name.trim().toLocaleLowerCase() ===
					trimmedDraftName.toLocaleLowerCase()
			)
	);
	const nameIsValid = $derived(
		trimmedDraftName.length > 0 && trimmedDraftName.length <= 200 && !nameAlreadyUsed
	);
	const manualDefinitionIsReady = $derived(
		selectedDefinition?.status === 'active' && selectedDefinition.trigger === 'manual'
	);
	const selectedRunFromList = $derived(
		selectedRunId ? runs.find((run) => run.id === selectedRunId) : undefined
	);
	const selectedRunQuery = createGetAutomationRun(
		() => projectId,
		() => selectedRunId ?? '',
		() => ({ query: { enabled: selectedRunId !== null } })
	);
	const selectedRun = $derived(selectedRunQuery.data?.data ?? selectedRunFromList);
	const queryError = $derived(definitionsQuery.error?.message ?? runsQuery.error?.message);
	const configurationError = $derived(
		configureMutation.error ? configurationErrorMessage(configureMutation.error) : null
	);
	const selectedRunError = $derived(
		selectedRunQuery.error ? selectedRunQuery.error.message : null
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
			}),
			selectedRunId
				? queryClient.invalidateQueries({
						queryKey: getGetAutomationRunQueryKey(projectId, selectedRunId)
					})
				: Promise.resolve()
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

	function updateName(event: Event): void {
		if (editorMode !== 'add') return;
		const target = event.currentTarget;
		if (!(target instanceof HTMLInputElement)) return;
		localDraft = { ...draft, name: target.value };
		feedback = null;
	}

	function selectDefinition(value: string | undefined): void {
		if (!value) return;
		const definition = supportedDefinitions.find((candidate) => candidate.id === value);
		if (!definition) return;
		editorMode = 'edit';
		selectedDefinitionId = definition.id;
		localDraft = null;
		feedback = null;
		manualError = null;
	}

	function startAddingDefinition(): void {
		editorMode = 'add';
		selectedDefinitionId = null;
		localDraft = { ...DEFAULT_AUTOMATION_DRAFT };
		feedback = null;
		manualError = null;
	}

	function updateTrigger(value: string | undefined): void {
		if (!isAutomationTrigger(value)) return;
		localDraft = { ...draft, trigger: value };
		feedback = null;
	}

	function updateStatus(value: string | undefined): void {
		if (!isAutomationStatus(value)) return;
		localDraft = { ...draft, status: value };
		feedback = null;
	}

	async function saveConfiguration(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (
			!nameIsValid ||
			configureMutation.isPending ||
			(editorMode === 'edit' && !selectedDefinition)
		)
			return;

		const addingDefinition = editorMode === 'add';
		feedback = null;
		manualError = null;
		try {
			const response = await configureMutation.mutateAsync({
				projectId,
				recipe: AUTOMATION_RECIPE_ROUTE,
				data: {
					name: trimmedDraftName,
					trigger: draft.trigger,
					status: draft.status
				}
			});
			await refreshAutomationQueries();
			localDraft = null;
			editorMode = 'edit';
			selectedDefinitionId = response.data.id;
			feedback = addingDefinition
				? 'Definition created and selected.'
				: 'Definition settings saved.';
		} catch {
			// The mutation error is rendered with the form.
		}
	}

	async function runManually(): Promise<void> {
		const definition = selectedDefinition;
		if (!definition || !manualDefinitionIsReady || manualPending) return;

		manualPending = true;
		feedback = null;
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
		} catch (error: unknown) {
			manualError = error;
		} finally {
			manualPending = false;
		}
	}

	function selectRun(runId: string): void {
		selectedRunId = selectedRunId === runId ? null : runId;
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

	function stepError(step: AutomationRunDto['steps'][number]): string {
		return step.error ?? '—';
	}

	function stepLabel(step: AutomationRunDto['steps'][number]): string {
		return `${step.ordinal + 1}. ${step.key}`;
	}
</script>

<div class="mx-auto flex w-full max-w-7xl flex-col gap-6 p-4 md:p-8">
	<header class="flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
		<div class="flex flex-col gap-2">
			<div class="flex items-center gap-2 text-sm text-muted-foreground">
				<Settings2Icon aria-hidden={true} />
				Project workspace / automation center
			</div>
			<h1 class="text-3xl font-semibold tracking-tight">Automation Center</h1>
			<p class="max-w-2xl text-muted-foreground">
				Configure the closed built-in maintenance recipe, launch eligible manual runs, and
				inspect job, step, and usage visibility.
			</p>
		</div>
		<Button
			variant="outline"
			disabled={definitionsQuery.isFetching || runsQuery.isFetching}
			onclick={() => void refreshAutomationQueries()}
		>
			{#if definitionsQuery.isFetching || runsQuery.isFetching}<Spinner
					data-icon="inline-start"
				/>{:else}<RefreshCwIcon data-icon="inline-start" />{/if}
			Refresh
		</Button>
	</header>

	{#if queryError}
		<Alert.Root variant="destructive" data-testid="automation-query-error">
			<CircleAlertIcon />
			<Alert.Title>Automation data unavailable</Alert.Title>
			<Alert.Description>{queryErrorMessage(queryError)}</Alert.Description>
			<Button variant="outline" size="sm" onclick={() => void retryQueries()}>Retry</Button>
		</Alert.Root>
	{/if}

	{#if feedback}
		<Alert.Root data-testid="automation-success">
			<CircleCheckIcon />
			<Alert.Title>Automation Center updated</Alert.Title>
			<Alert.Description>{feedback}</Alert.Description>
		</Alert.Root>
	{/if}

	<div class="grid gap-6 xl:grid-cols-[minmax(0,0.9fr)_minmax(0,1.4fr)]">
		<Card.Root>
			<Card.Header>
				<div class="flex items-start justify-between gap-3">
					<div class="flex flex-col gap-1.5">
						<Card.Title>Built-in recipe</Card.Title>
						<Card.Description>
							Only the supported project maintenance recipe can be configured here.
						</Card.Description>
					</div>
					<Badge variant={supportedDefinitions.length > 0 ? 'secondary' : 'outline'}>
						{supportedDefinitions.length > 0 ? 'Configured' : 'Not configured'}
					</Badge>
				</div>
			</Card.Header>
			<Card.Content class="flex flex-col gap-5">
				<Field.FieldGroup data-testid="automation-definition-editor">
					<Field.Field>
						<Field.FieldLabel for="automation-definition">Definition</Field.FieldLabel>
						<div class="flex flex-col gap-2 sm:flex-row sm:items-center">
							<Select.Root
								type="single"
								value={selectedDefinitionId ?? ''}
								onValueChange={selectDefinition}
							>
								<Select.Trigger
									id="automation-definition"
									class="w-full sm:flex-1"
									data-testid="automation-definition-select"
								>
									{selectedDefinition?.name ?? 'Select a definition to edit'}
								</Select.Trigger>
								<Select.Content>
									<Select.Group>
										<Select.Label>Project maintenance definitions</Select.Label>
										{#each supportedDefinitions as definition (definition.id)}
											<Select.Item
												value={definition.id}
												label={`${definition.name} · ${labelForStatus(definition.status)} · ${labelForTrigger(definition.trigger)}`}
											>
												{definition.name} · {labelForStatus(
													definition.status
												)} · {labelForTrigger(definition.trigger)}
											</Select.Item>
										{/each}
									</Select.Group>
								</Select.Content>
							</Select.Root>
							<Button
								type="button"
								variant={editorMode === 'add' ? 'default' : 'outline'}
								onclick={startAddingDefinition}
								data-testid="automation-add-definition"
							>
								Add definition
							</Button>
						</div>
						<Field.FieldDescription>
							{#if editorMode === 'edit'}
								Editing the selected definition. Its name is its identity and cannot
								be renamed.
							{:else}
								Add a named definition or select an existing one to edit its trigger
								and status.
							{/if}
						</Field.FieldDescription>
					</Field.Field>
				</Field.FieldGroup>

				{#if unsupportedDefinitions.length > 0}
					<Alert.Root data-testid="automation-unsupported-definitions">
						<CircleAlertIcon />
						<Alert.Title>Unsupported definitions ignored</Alert.Title>
						<Alert.Description>
							Only project_maintenance v1 definitions can be managed here. The
							following definitions remain untouched:
						</Alert.Description>
						<ul class="flex flex-col gap-1 text-sm">
							{#each unsupportedDefinitions as definition (definition.id)}
								<li>
									{definition.name} · {definition.recipe} v{definition.version}
								</li>
							{/each}
						</ul>
					</Alert.Root>
				{/if}

				<div
					class="rounded-lg border bg-muted/30 p-3 text-sm"
					data-testid="automation-recipe"
				>
					<div class="flex flex-wrap items-center justify-between gap-2">
						<span class="font-medium">{AUTOMATION_RECIPE_ROUTE}</span>
						<span class="text-muted-foreground"
							>Recipe {AUTOMATION_RECIPE_ID} · Version {AUTOMATION_RECIPE_VERSION}</span
						>
					</div>
					<p class="mt-2 text-muted-foreground">
						Steps are built into this recipe and are not editable from the Automation
						Center.
					</p>
					<ol class="mt-3 flex flex-col gap-2">
						{#each selectedDefinition?.steps ?? BUILT_IN_STEPS as step (step.key)}
							<li
								class="flex items-center justify-between gap-3 rounded-md bg-background px-3 py-2"
							>
								<span>{step.ordinal + 1}. {step.key}</span>
								<Badge variant="outline">{labelForStatus(step.kind)}</Badge>
							</li>
						{/each}
					</ol>
				</div>

				<form class="flex flex-col gap-5" onsubmit={saveConfiguration}>
					<Field.FieldGroup>
						<Field.Field data-invalid={!nameIsValid}>
							<Field.FieldLabel for="automation-name">Name</Field.FieldLabel>
							<Input
								id="automation-name"
								value={draft.name}
								readonly={editorMode === 'edit'}
								maxlength={200}
								oninput={updateName}
								aria-invalid={!nameIsValid}
								aria-readonly={editorMode === 'edit'}
								placeholder="Project maintenance"
							/>
							<Field.FieldDescription>
								{#if editorMode === 'edit'}
									Definition names are immutable after creation.
								{:else}
									Use a concise, unique name for this project's built-in
									automation.
								{/if}
							</Field.FieldDescription>
							{#if !nameIsValid}
								<Field.FieldError
									>{nameAlreadyUsed
										? 'A definition with this name already exists.'
										: 'Name is required and must be at most 200 characters.'}</Field.FieldError
								>
							{/if}
						</Field.Field>

						<Field.FieldSet>
							<Field.FieldLegend>Trigger</Field.FieldLegend>
							<Field.FieldDescription>
								Choose one of the supported domain events or a manual trigger.
							</Field.FieldDescription>
							<ToggleGroup.Root
								type="single"
								value={draft.trigger}
								variant="outline"
								class="flex w-full flex-wrap justify-start"
								onValueChange={updateTrigger}
								aria-label="Automation trigger"
							>
								{#each AUTOMATION_TRIGGERS as trigger (trigger)}
									<ToggleGroup.Item
										value={trigger}
										class="grow sm:grow-0"
										data-testid={`automation-trigger-${trigger}`}
									>
										{labelForTrigger(trigger)}
									</ToggleGroup.Item>
								{/each}
							</ToggleGroup.Root>
						</Field.FieldSet>

						<Field.FieldSet>
							<Field.FieldLegend>Status</Field.FieldLegend>
							<Field.FieldDescription>
								Paused definitions remain visible but cannot be manually started.
							</Field.FieldDescription>
							<ToggleGroup.Root
								type="single"
								value={draft.status}
								variant="outline"
								onValueChange={updateStatus}
								aria-label="Automation status"
							>
								{#each AUTOMATION_STATUSES as status (status)}
									<ToggleGroup.Item
										value={status}
										data-testid={`automation-status-${status}`}
									>
										{labelForStatus(status)}
									</ToggleGroup.Item>
								{/each}
							</ToggleGroup.Root>
						</Field.FieldSet>
					</Field.FieldGroup>

					{#if configurationError}
						<Alert.Root variant="destructive" data-testid="automation-config-error">
							<Alert.Title>Could not save recipe settings</Alert.Title>
							<Alert.Description>{configurationError}</Alert.Description>
						</Alert.Root>
					{/if}

					<Button
						type="submit"
						data-testid="automation-save-definition"
						disabled={!nameIsValid ||
							configureMutation.isPending ||
							(editorMode === 'edit' && !selectedDefinition)}
					>
						{#if configureMutation.isPending}<Spinner
								data-icon="inline-start"
							/>{:else}<SaveIcon data-icon="inline-start" />{/if}
						{configureMutation.isPending
							? 'Saving…'
							: editorMode === 'add'
								? 'Add definition'
								: 'Save definition settings'}
					</Button>
				</form>
			</Card.Content>
		</Card.Root>

		<Card.Root>
			<Card.Header>
				<div class="flex flex-wrap items-center justify-between gap-3">
					<div class="flex flex-col gap-1.5">
						<Card.Title>Manual execution</Card.Title>
						<Card.Description>
							Manual execution is available only when this recipe is active and uses
							the manual trigger.
						</Card.Description>
					</div>
					<Button
						disabled={!manualDefinitionIsReady || manualPending}
						onclick={() => void runManually()}
						data-testid="automation-run-manually"
					>
						{#if manualPending}<Spinner data-icon="inline-start" />{:else}<PlayIcon
								data-icon="inline-start"
							/>{/if}
						{manualPending ? 'Starting…' : 'Run manually'}
					</Button>
				</div>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4">
				<div class="rounded-lg border p-3 text-sm" data-testid="automation-manual-state">
					{#if !selectedDefinition && supportedDefinitions.length === 0}
						Add a definition above before starting a run.
					{:else if !selectedDefinition}
						Select an active manual definition above before starting a run.
					{:else if selectedDefinition.status === 'paused'}
						This definition is paused. Activate it before starting a manual run.
					{:else if selectedDefinition.trigger !== 'manual'}
						This definition listens for <span class="font-medium"
							>{labelForTrigger(selectedDefinition.trigger)}</span
						>. Select Manual above to enable the button.
					{:else}
						The run request uses a fresh idempotency key for every click.
					{/if}
				</div>
				{#if manualError}
					<Alert.Root variant="destructive" data-testid="automation-manual-error">
						<Alert.Title>Could not start automation</Alert.Title>
						<Alert.Description>{manualErrorMessage(manualError)}</Alert.Description>
					</Alert.Root>
				{/if}
			</Card.Content>
		</Card.Root>
	</div>

	<section aria-labelledby="automation-runs-heading" class="flex flex-col gap-4">
		<div class="flex flex-wrap items-end justify-between gap-2">
			<div class="flex flex-col gap-1">
				<h2 id="automation-runs-heading" class="text-xl font-semibold">Recent runs</h2>
				<p class="text-sm text-muted-foreground">
					Showing the latest {RUN_LIST_PARAMS.limit} runs. Queued and running runs refresh every
					two seconds while this page is visible.
				</p>
			</div>
			<Badge variant="secondary">{runs.length} shown</Badge>
		</div>

		{#if runsQuery.isPending}
			<div class="grid gap-4" aria-label="Loading automation runs">
				{#each [0, 1] as skeleton (skeleton)}
					<Card.Root>
						<Card.Header class="gap-3">
							<Skeleton class="h-5 w-1/3" />
							<Skeleton class="h-4 w-2/3" />
						</Card.Header>
						<Card.Content class="flex flex-col gap-3">
							<Skeleton class="h-20 w-full" />
							<Skeleton class="h-16 w-full" />
						</Card.Content>
					</Card.Root>
				{/each}
			</div>
		{:else if runsQuery.error}
			<Empty.Root data-testid="automation-runs-error">
				<Empty.Media variant="icon"><CircleAlertIcon /></Empty.Media>
				<Empty.Header>
					<Empty.Title>Runs could not be loaded</Empty.Title>
					<Empty.Description>{queryErrorMessage(runsQuery.error)}</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button variant="outline" onclick={() => void runsQuery.refetch()}
						>Retry runs</Button
					>
				</Empty.Content>
			</Empty.Root>
		{:else if runs.length === 0}
			<Empty.Root data-testid="automation-runs-empty">
				<Empty.Media variant="icon"><PlayIcon /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No automation runs yet</Empty.Title>
					<Empty.Description>
						Configure the built-in recipe and start a manual run, or wait for its
						selected domain trigger.
					</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else}
			<div class="grid gap-4" data-testid="automation-runs">
				{#each runs as run (run.id)}
					<Card.Root data-testid="automation-run">
						<Card.Header class="gap-3">
							<div class="flex flex-wrap items-center justify-between gap-2">
								<Card.Title>Run {run.id.slice(0, 8)}</Card.Title>
								<Badge variant={badgeVariant(run.status)}
									>{labelForStatus(run.status)}</Badge
								>
							</div>
							<Card.Description>
								{run.recipe} · v{run.version} · {labelForTrigger(run.trigger)}
							</Card.Description>
						</Card.Header>
						<Card.Content class="flex flex-col gap-4">
							<dl
								class="grid gap-3 rounded-lg bg-muted/40 p-3 text-sm sm:grid-cols-2 lg:grid-cols-4"
							>
								<div>
									<dt class="text-muted-foreground">Created</dt>
									<dd class="font-medium">{formatTimestamp(run.created_at)}</dd>
								</div>
								<div>
									<dt class="text-muted-foreground">Finished</dt>
									<dd class="font-medium">{formatTimestamp(run.finished_at)}</dd>
								</div>
								<div>
									<dt class="text-muted-foreground">Input tokens</dt>
									<dd class="font-medium">
										{formatInteger(run.usage.input_tokens)}
									</dd>
								</div>
								<div>
									<dt class="text-muted-foreground">Output tokens</dt>
									<dd class="font-medium">
										{formatInteger(run.usage.output_tokens)}
									</dd>
								</div>
							</dl>

							<div class="grid gap-4 lg:grid-cols-2">
								<section
									class="flex flex-col gap-2 rounded-lg border p-3"
									aria-label="Automation job"
								>
									<div class="flex items-center justify-between gap-2">
										<h3 class="font-medium">Job</h3>
										<Badge variant={badgeVariant(run.job.status)}
											>{labelForStatus(run.job.status)}</Badge
										>
									</div>
									<dl class="grid gap-2 text-sm sm:grid-cols-2">
										<div>
											<dt class="text-muted-foreground">Attempts</dt>
											<dd>{run.job.attempts} / {run.job.max_attempts}</dd>
										</div>
										<div>
											<dt class="text-muted-foreground">Available</dt>
											<dd>{formatTimestamp(run.job.available_at)}</dd>
										</div>
									</dl>
									{#if run.job.last_error}
										<p class="text-sm text-destructive">{run.job.last_error}</p>
									{/if}
								</section>

								<section
									class="flex flex-col gap-2 rounded-lg border p-3"
									aria-label="Automation usage"
								>
									<h3 class="font-medium">Usage</h3>
									<p class="text-sm">{formatCostMicros(run.usage.cost_micros)}</p>
									{#if run.error}
										<p class="text-sm text-destructive">
											Run error: {run.error}
										</p>
									{/if}
								</section>
							</div>

							<section class="flex flex-col gap-2" aria-label="Automation steps">
								<h3 class="font-medium">Steps</h3>
								{#if run.steps.length === 0}
									<p class="text-sm text-muted-foreground">No steps reported.</p>
								{:else}
									<ol class="flex flex-col gap-2">
										{#each run.steps as step (step.id)}
											<li
												class="flex flex-col gap-1 rounded-lg border p-3 text-sm sm:flex-row sm:items-center sm:justify-between sm:gap-3"
												data-testid="automation-step"
											>
												<div class="flex min-w-0 flex-col gap-1">
													<span class="truncate font-medium"
														>{stepLabel(step)}</span
													>
													<span class="text-xs text-muted-foreground"
														>{labelForStatus(step.kind)}</span
													>
												</div>
												<div class="flex flex-wrap items-center gap-2">
													<Badge variant={badgeVariant(step.status)}
														>{labelForStatus(step.status)}</Badge
													>
													<span class="text-xs text-muted-foreground"
														>{step.attempts} attempt{step.attempts === 1
															? ''
															: 's'}</span
													>
												</div>
												{#if step.error}
													<p class="basis-full text-xs text-destructive">
														{stepError(step)}
													</p>
												{/if}
											</li>
										{/each}
									</ol>
								{/if}
							</section>

							<div class="flex justify-end">
								<Button
									variant="outline"
									size="sm"
									aria-expanded={selectedRunId === run.id}
									onclick={() => selectRun(run.id)}
								>
									<EyeIcon data-icon="inline-start" />
									{selectedRunId === run.id
										? 'Hide run details'
										: 'View run details'}
								</Button>
							</div>
						</Card.Content>
					</Card.Root>
				{/each}
			</div>
		{/if}
	</section>

	{#if selectedRunId}
		<Card.Root data-testid="automation-run-details">
			<Card.Header>
				<Card.Title>Run details</Card.Title>
				<Card.Description>{selectedRunId}</Card.Description>
			</Card.Header>
			<Card.Content>
				{#if selectedRunQuery.isPending}
					<div class="flex flex-col gap-3" aria-label="Loading automation run details">
						<Skeleton class="h-5 w-1/3" />
						<Skeleton class="h-16 w-full" />
					</div>
				{:else if selectedRunError}
					<Alert.Root variant="destructive">
						<Alert.Title>Run details unavailable</Alert.Title>
						<Alert.Description>{selectedRunError}</Alert.Description>
					</Alert.Root>
				{:else if selectedRun}
					<div class="flex flex-wrap items-center gap-2 text-sm">
						<Badge variant={badgeVariant(selectedRun.status)}
							>{labelForStatus(selectedRun.status)}</Badge
						>
						<span class="text-muted-foreground"
							>{labelForTrigger(selectedRun.trigger)} trigger · created
							{formatTimestamp(selectedRun.created_at)}</span
						>
					</div>
				{:else}
					<p class="text-sm text-muted-foreground">This run is no longer available.</p>
				{/if}
			</Card.Content>
		</Card.Root>
	{/if}
</div>

<style>
	:global([data-testid='automation-run'] dd) {
		word-break: break-word;
	}
</style>
