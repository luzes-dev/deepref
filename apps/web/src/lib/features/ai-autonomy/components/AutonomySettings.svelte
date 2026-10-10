<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Lock } from '@lucide/svelte';
	import {
		createGetAiAutonomy,
		createGetAiBudget,
		createGetAiStatus,
		createUpdateAiAutonomy,
		createUpdateAiBudget,
		getGetAiAutonomyQueryKey,
		getGetAiBudgetQueryKey
	} from '#lib/api/generated/ai/ai.js';
	import * as Alert from '@deepref/ui/alert';
	import { Badge } from '@deepref/ui/badge';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import { Progress } from '@deepref/ui/progress';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as ToggleGroup from '@deepref/ui/toggle-group';
	import { StatePanel } from '@deepref/ui/layout';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import { isAiPaused, parseBudgetInput } from '../budget';
	import {
		FUZZY_AUTO_MERGE_SCORE,
		LEVEL_EXPLANATION,
		LEVEL_LABEL,
		LEVEL_ORDER,
		formatUsd,
		isAutonomyLevel
	} from '../levels';

	let { projectId }: { projectId: string } = $props();

	const BUDGET_SAVE_DELAY_MS = 600;

	const queryClient = useQueryClient();
	const autonomyQuery = createGetAiAutonomy(() => projectId);
	const budgetQuery = createGetAiBudget(() => projectId);
	const statusQuery = createGetAiStatus(() => ({ project_id: projectId }));
	const updateAutonomy = createUpdateAiAutonomy();
	const updateBudget = createUpdateAiBudget();

	const tasks = $derived(autonomyQuery.data?.data.tasks ?? []);
	const locked = $derived(autonomyQuery.data?.data.locked ?? []);
	const budget = $derived(budgetQuery.data?.data);
	const status = $derived(statusQuery.data?.data);
	let savingTask = $state<string | null>(null);
	let budgetDraft = $state<string | null>(null);
	let budgetError = $state('');
	let budgetSaveState = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
	let budgetTimer: ReturnType<typeof setTimeout> | undefined;
	let pendingBudget: { amount: number; draft: string } | null = null;
	let budgetRequest = 0;

	const spentPercent = $derived(
		budget && budget.monthly_budget_usd > 0
			? Math.min(100, Math.round((budget.spent_usd / budget.monthly_budget_usd) * 100))
			: budget?.exhausted
				? 100
				: 0
	);
	// The amount the AI will use: what is typed when it is valid, otherwise what is saved.
	const effectiveBudget = $derived.by(() => {
		if (budgetDraft !== null) {
			const parsed = parseBudgetInput(budgetDraft);
			if (parsed.ok) return parsed.amount;
		}
		return budget?.monthly_budget_usd;
	});
	const paused = $derived(effectiveBudget !== undefined && isAiPaused(effectiveBudget));
	const budgetStatusLabel = $derived(
		budgetSaveState === 'saving'
			? 'Saving…'
			: budgetSaveState === 'saved'
				? 'Saved'
				: budgetSaveState === 'error'
					? 'Not saved. Check the amount and try again.'
					: ''
	);
	const modelStatus = $derived.by(() => {
		if (!status) return undefined;
		if (!status.configured) {
			return {
				label: 'Not configured',
				variant: 'secondary' as const,
				detail: 'No AI provider is set up on this server, so AI work is unavailable.'
			};
		}
		if (!status.suggestions_available) {
			return {
				label: 'Configured, not available',
				variant: 'warning' as const,
				detail: 'A provider is set up, but AI suggestions are switched off on this server.'
			};
		}
		return {
			label: 'Available',
			variant: 'success' as const,
			detail: 'AI suggestions and automatic work can run for this project.'
		};
	});
	const fuzzyAutoMergePercent = Math.round(FUZZY_AUTO_MERGE_SCORE * 100);

	async function chooseLevel(task: string, value: string | undefined): Promise<void> {
		if (!value || !isAutonomyLevel(value) || savingTask) return;
		const current = tasks.find((item) => item.task === task);
		if (!current || current.level === value) return;
		savingTask = task;
		try {
			const response = await updateAutonomy.mutateAsync({
				projectId,
				data: { changes: [{ task, level: value }] }
			});
			queryClient.setQueryData(getGetAiAutonomyQueryKey(projectId), response);
		} catch (error) {
			notifyError('Could not save this setting', error, 'The AI setting was not saved.');
			await autonomyQuery.refetch();
		} finally {
			savingTask = null;
		}
	}

	function onBudgetInput(value: string): void {
		budgetDraft = value;
		const parsed = parseBudgetInput(value);
		budgetError = parsed.ok ? '' : parsed.message;
		clearTimeout(budgetTimer);
		pendingBudget = parsed.ok ? { amount: parsed.amount, draft: value } : null;
		if (!parsed.ok) {
			budgetSaveState = 'idle';
			return;
		}
		budgetTimer = setTimeout(flushPendingBudget, BUDGET_SAVE_DELAY_MS);
	}

	// Saves a debounced budget immediately. Also runs when the settings close, so a
	// value typed just before closing is not lost.
	function flushPendingBudget(): void {
		clearTimeout(budgetTimer);
		const pending = pendingBudget;
		pendingBudget = null;
		if (pending) void saveBudget(pending.amount, pending.draft);
	}

	async function saveBudget(amount: number, draft: string): Promise<void> {
		const request = ++budgetRequest;
		budgetSaveState = 'saving';
		try {
			const response = await updateBudget.mutateAsync({
				projectId,
				data: { monthly_budget_usd: amount }
			});
			// A newer edit or save has started; its response is the one that counts.
			if (request !== budgetRequest) return;
			queryClient.setQueryData(getGetAiBudgetQueryKey(projectId), response);
			if (budgetDraft === draft) budgetDraft = null;
			budgetSaveState = 'saved';
		} catch (error) {
			if (request !== budgetRequest) return;
			budgetSaveState = 'error';
			notifyError('Could not save the budget', error, 'The monthly budget was not saved.');
		}
	}

	$effect(() => () => flushPendingBudget());
</script>

<div class="flex flex-col gap-8" data-testid="ai-settings">
	<section class="flex flex-col gap-3" aria-labelledby="ai-model-title">
		<div>
			<h2 id="ai-model-title" class="text-sm font-semibold">AI model</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Chosen by your workspace administrator on the server. Read only.
			</p>
		</div>
		{#if statusQuery.isPending}
			<Skeleton class="h-16 w-full" />
		{:else if statusQuery.error}
			<p class="text-xs text-muted-foreground" data-testid="ai-model-unavailable">
				The AI model status could not be loaded.
			</p>
		{:else if modelStatus && status}
			<dl
				class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[auto_1fr]"
				data-testid="ai-model-status"
			>
				<dt class="text-muted-foreground">Provider</dt>
				<dd>{status.provider ?? 'Not set'}</dd>
				<dt class="text-muted-foreground">Model</dt>
				<dd>{status.model ?? 'Not set'}</dd>
				<dt class="text-muted-foreground">Status</dt>
				<dd class="flex flex-col items-start gap-1">
					<Badge variant={modelStatus.variant} size="sm">{modelStatus.label}</Badge>
					<span class="text-xs text-muted-foreground">{modelStatus.detail}</span>
				</dd>
			</dl>
		{/if}
	</section>

	<section class="flex flex-col gap-3" aria-labelledby="ai-autonomy-title">
		<div>
			<h2 id="ai-autonomy-title" class="text-sm font-semibold">How far the AI may go</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				Choose per kind of work. These settings apply to this project only.
			</p>
		</div>
		{#if autonomyQuery.isPending}
			<Skeleton class="h-40 w-full" />
		{:else if autonomyQuery.error}
			<StatePanel
				state="error"
				title="AI settings unavailable"
				description={autonomyQuery.error.message}
			/>
		{:else}
			<ul class="flex flex-col" data-testid="ai-autonomy-list">
				{#each tasks as item (item.task)}
					{@const level = isAutonomyLevel(item.level) ? item.level : 'suggest'}
					<li
						class="flex flex-col gap-2 border-b py-3"
						data-testid={`ai-task-${item.task}`}
					>
						<div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
							<div class="min-w-0">
								<p class="text-sm font-medium">{item.label}</p>
								<p class="text-xs text-muted-foreground">{item.description}</p>
							</div>
							<ToggleGroup.Root
								type="single"
								variant="outline"
								size="sm"
								value={level}
								disabled={savingTask === item.task}
								onValueChange={(value) => void chooseLevel(item.task, value)}
								aria-label={`${item.label}: how far the AI may go`}
							>
								{#each LEVEL_ORDER as option (option)}
									{#if item.allowed_levels.includes(option)}
										<ToggleGroup.Item
											value={option}
											aria-label={LEVEL_LABEL[option]}
										>
											{LEVEL_LABEL[option]}
										</ToggleGroup.Item>
									{/if}
								{/each}
							</ToggleGroup.Root>
						</div>
						<p class="text-xs text-muted-foreground">
							{LEVEL_EXPLANATION[level]}
							{#if item.is_default}<Badge variant="outline" size="sm" class="ml-1"
									>Default</Badge
								>{/if}
						</p>
						{#if item.task === 'fuzzy_duplicates' && level === 'act'}
							<p
								class="text-xs text-muted-foreground"
								data-testid="ai-task-fuzzy-act-note"
							>
								Near-identical matches (about {fuzzyAutoMergePercent}% or more)
								merge automatically. The rest stay suggestions.
							</p>
						{/if}
					</li>
				{/each}
				{#each locked as item (item.task)}
					<li
						class="flex flex-wrap items-center justify-between gap-x-4 gap-y-1 border-b py-3"
						data-testid={`ai-locked-${item.task}`}
					>
						<div class="min-w-0">
							<p class="text-sm font-medium">{item.label}</p>
							<p class="text-xs text-muted-foreground">{item.description}</p>
						</div>
						<Badge variant="secondary"
							><Lock data-icon="inline-start" />Always you</Badge
						>
					</li>
				{/each}
			</ul>
			<details class="disclosure">
				<summary>What do these levels mean?</summary>
				<dl class="grid gap-2 pt-2 text-xs">
					{#each LEVEL_ORDER as option (option)}
						<div>
							<dt class="font-medium">{LEVEL_LABEL[option]}</dt>
							<dd class="text-muted-foreground">{LEVEL_EXPLANATION[option]}</dd>
						</div>
					{/each}
				</dl>
			</details>
		{/if}
	</section>

	<section class="flex flex-col gap-3" aria-labelledby="ai-budget-title">
		<div>
			<h2 id="ai-budget-title" class="text-sm font-semibold">Monthly AI budget</h2>
			<p class="mt-1 text-xs text-muted-foreground">
				When the budget is used up, the AI pauses until next month or until you raise it.
			</p>
		</div>
		{#if budgetQuery.isPending}
			<Skeleton class="h-16 w-full" />
		{:else if budget}
			<div class="flex flex-col gap-2" data-testid="ai-budget">
				{#if paused}
					<Alert.Root variant="warning" data-testid="ai-budget-paused">
						<Alert.Title>AI is paused for this project</Alert.Title>
						<Alert.Description>
							The monthly budget is $0. Raise it to let the AI run again.
						</Alert.Description>
					</Alert.Root>
				{/if}
				<div class="flex items-baseline justify-between gap-3 text-sm">
					<span class="tabular-nums"
						>{formatUsd(budget.spent_usd)} used this month of {formatUsd(
							budget.monthly_budget_usd
						)}</span
					>
					{#if budget.exhausted}<Badge variant="warning">Budget reached</Badge>{/if}
				</div>
				<Progress value={spentPercent} aria-label="Share of the monthly AI budget used" />
				<Field.Field class="mt-2 max-w-xs">
					<Field.FieldLabel for="ai-budget-input">Budget per month (US$)</Field.FieldLabel
					>
					<Input
						id="ai-budget-input"
						type="number"
						min="0"
						step="0.5"
						inputmode="decimal"
						value={budgetDraft ?? String(budget.monthly_budget_usd)}
						oninput={(event) => onBudgetInput(event.currentTarget.value)}
						aria-invalid={budgetError ? 'true' : undefined}
						aria-describedby="ai-budget-status"
					/>
					{#if budgetError}<Field.FieldError>{budgetError}</Field.FieldError>{/if}
					<p
						id="ai-budget-status"
						class="text-xs text-muted-foreground"
						role={budgetSaveState === 'error' ? 'alert' : 'status'}
						aria-live="polite"
					>
						{budgetStatusLabel}
					</p>
				</Field.Field>
			</div>
		{/if}
	</section>
</div>
