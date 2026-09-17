<script lang="ts">
	import { Database, Globe, Palette } from '@lucide/svelte';
	import type { SettingsDto, UpdateSettings } from '$lib/api/generated/models';
	import { createGetSettings, createUpdateSettings } from '$lib/api/generated/settings/settings';
	import { StatePanel, Surface } from '@deepref/ui/layout';
	import { notifyError } from '$lib/features/notifications/toast';
	import { Button } from '@deepref/ui/button';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import { Separator } from '@deepref/ui/separator';
	import { Spinner } from '@deepref/ui/spinner';
	import ThemeSelector from '@deepref/ui/theme-selector';

	type NumericSettingKey =
		'default_max_depth' | 'max_concurrency' | 'rate_limit_per_second' | 'retry_attempts';
	type DraftSettings = {
		crossref_mailto: string;
		default_max_depth: string;
		max_concurrency: string;
		rate_limit_per_second: string;
		retry_attempts: string;
	};
	type DraftKey = keyof DraftSettings;
	type ValidationErrors = Partial<Record<DraftKey, string>>;
	type ValidationResult =
		{ ok: true; data: UpdateSettings } | { ok: false; errors: ValidationErrors };
	type SaveStatus = 'ready' | 'dirty' | 'saving' | 'saved' | 'error';

	const numericFields: Record<NumericSettingKey, { label: string; minimum: number }> = {
		default_max_depth: { label: 'Default max depth', minimum: 0 },
		max_concurrency: { label: 'Max concurrency', minimum: 1 },
		rate_limit_per_second: { label: 'Rate limit per second', minimum: 1 },
		retry_attempts: { label: 'Retry attempts', minimum: 1 }
	};
	const numericFieldKeys: readonly NumericSettingKey[] = [
		'default_max_depth',
		'max_concurrency',
		'rate_limit_per_second',
		'retry_attempts'
	];

	const settingsQueryResult = createGetSettings();
	const updateSettings = createUpdateSettings();

	let activeTab = $state<'ingestion' | 'appearance' | 'providers'>('ingestion');
	let submittedSettings = $state<SettingsDto | null>(null);
	let edits = $state<Partial<DraftSettings>>({});
	let validationErrors = $state<ValidationErrors>({});

	const serverSettings = $derived(submittedSettings ?? settingsQueryResult.data?.data);
	const baseline = $derived(serverSettings ? toDraft(serverSettings) : null);
	const draft = $derived.by(() => ({ ...emptyDraft(), ...(baseline ?? {}), ...edits }));
	const isLoading = $derived(settingsQueryResult.isPending && !serverSettings);
	const loadError = $derived(
		!settingsQueryResult.data ? (settingsQueryResult.error?.message ?? '') : ''
	);
	const isDirty = $derived(baseline !== null && !sameDraft(draft, baseline));
	const hasValidationErrors = $derived(Object.keys(validationErrors).length > 0);
	const saveStatus = $derived<SaveStatus>(
		updateSettings.isPending
			? 'saving'
			: updateSettings.error
				? 'error'
				: updateSettings.isSuccess && !isDirty
					? 'saved'
					: isDirty
						? 'dirty'
						: 'ready'
	);
	const saveStatusLabel = $derived(
		(
			{
				ready: 'Ready to edit',
				dirty: 'Unsaved changes',
				saving: 'Saving changes',
				saved: 'Changes saved',
				error: 'Save failed'
			} satisfies Record<SaveStatus, string>
		)[saveStatus]
	);

	function emptyDraft(): DraftSettings {
		return {
			crossref_mailto: '',
			default_max_depth: '0',
			max_concurrency: '1',
			rate_limit_per_second: '1',
			retry_attempts: '1'
		};
	}

	function toDraft(settings: SettingsDto): DraftSettings {
		return {
			crossref_mailto: settings.crossref_mailto,
			default_max_depth: String(settings.default_max_depth),
			max_concurrency: String(settings.max_concurrency),
			rate_limit_per_second: String(settings.rate_limit_per_second),
			retry_attempts: String(settings.retry_attempts)
		};
	}

	function sameDraft(left: DraftSettings, right: DraftSettings): boolean {
		return (
			left.crossref_mailto === right.crossref_mailto &&
			left.default_max_depth === right.default_max_depth &&
			left.max_concurrency === right.max_concurrency &&
			left.rate_limit_per_second === right.rate_limit_per_second &&
			left.retry_attempts === right.retry_attempts
		);
	}

	function numericError(key: NumericSettingKey, rawValue: string): string | undefined {
		const field = numericFields[key];
		const value = Number(rawValue);
		if (rawValue.trim() === '' || !Number.isInteger(value) || value < field.minimum) {
			return `${field.label} must be an integer of at least ${field.minimum}.`;
		}
		return undefined;
	}

	function fieldError(key: DraftKey, value: string): string | undefined {
		if (key === 'crossref_mailto') {
			return value.trim() === '' ? 'Crossref mailto is required.' : undefined;
		}
		return numericError(key, value);
	}

	function setDraft<Key extends DraftKey>(key: Key, value: DraftSettings[Key]): void {
		edits = { ...edits, [key]: value };
		const error = fieldError(key, value);
		const nextErrors = { ...validationErrors };
		if (error) nextErrors[key] = error;
		else delete nextErrors[key];
		validationErrors = nextErrors;
		updateSettings.reset();
	}

	function validateDraft(value: DraftSettings): ValidationResult {
		const errors: ValidationErrors = {};
		const mailto = value.crossref_mailto.trim();
		if (!mailto) errors.crossref_mailto = 'Crossref mailto is required.';

		const parsed: Partial<Record<NumericSettingKey, number>> = {};
		for (const key of numericFieldKeys) {
			const error = numericError(key, value[key]);
			if (error) errors[key] = error;
			else parsed[key] = Number(value[key]);
		}

		if (Object.keys(errors).length > 0) return { ok: false, errors };
		return {
			ok: true,
			data: {
				crossref_mailto: mailto,
				default_max_depth: parsed.default_max_depth ?? 0,
				max_concurrency: parsed.max_concurrency ?? 1,
				rate_limit_per_second: parsed.rate_limit_per_second ?? 1,
				retry_attempts: parsed.retry_attempts ?? 1
			}
		};
	}

	async function save(): Promise<void> {
		const result = validateDraft(draft);
		if (!result.ok) {
			validationErrors = result.errors;
			return;
		}

		try {
			const response = await updateSettings.mutateAsync({ data: result.data });
			submittedSettings = response.data;
			edits = {};
			validationErrors = {};
		} catch (error) {
			notifyError(
				'Could not save settings',
				error,
				'The application settings request failed.'
			);
		}
	}
</script>

<svelte:head>
	<title>Settings · DeepRef</title>
	<meta
		name="description"
		content="Configure application-wide ingestion and evidence provider defaults."
	/>
</svelte:head>

<div class="flex h-full min-h-0 flex-col overflow-auto bg-background" data-testid="settings-page">
	<div class="mx-auto flex w-full max-w-5xl flex-col gap-6 p-4 sm:p-6 lg:p-8">
		<div class="flex items-start justify-between gap-4">
			<div class="space-y-0.5">
				<h1 class="text-2xl font-bold tracking-tight">Settings</h1>
				<p class="text-muted-foreground">
					Manage your application settings and review ingestion preferences.
				</p>
			</div>
			<ThemeSelector />
		</div>
		<Separator />

		{#if isLoading}
			<div data-testid="settings-loading">
				<div class="flex flex-col space-y-8 lg:flex-row lg:space-y-0 lg:space-x-12">
					<aside class="-mx-4 lg:w-1/5">
						<nav
							class="flex space-x-2 px-4 lg:flex-col lg:space-y-1 lg:space-x-0 lg:px-0"
						>
							<Button
								variant="ghost"
								class="justify-start gap-2 text-sm font-medium"
								disabled
							>
								<Database class="size-4" />
								Ingestion
							</Button>
						</nav>
					</aside>
					<div class="flex-1 lg:max-w-3xl">
						<Surface tone="subtle" class="p-6">
							<StatePanel
								state="loading"
								title="Loading application settings"
								description="Retrieving the current provider and ingestion defaults."
							/>
						</Surface>
					</div>
				</div>
			</div>
		{:else if loadError}
			<div data-testid="settings-load-error">
				<div class="flex flex-col space-y-8 lg:flex-row lg:space-y-0 lg:space-x-12">
					<div class="flex-1">
						<Surface tone="subtle" class="p-6">
							<StatePanel
								state="error"
								title="Settings unavailable"
								description={loadError}
							>
								{#snippet action()}
									<Button
										variant="outline"
										onclick={() => void settingsQueryResult.refetch()}
									>
										Try again
									</Button>
								{/snippet}
							</StatePanel>
						</Surface>
					</div>
				</div>
			</div>
		{:else if baseline}
			<div class="flex flex-col space-y-8 lg:flex-row lg:space-y-0 lg:space-x-12">
				<!-- Sub-navigation inspired by shadcn-svelte Forms example -->
				<aside class="-mx-4 lg:w-1/5">
					<nav
						class="flex space-x-2 overflow-x-auto px-4 lg:flex-col lg:space-y-1 lg:space-x-0 lg:px-0"
						aria-label="Settings navigation"
					>
						<Button
							variant={activeTab === 'ingestion' ? 'secondary' : 'ghost'}
							class="justify-start gap-2 text-sm font-medium"
							onclick={() => (activeTab = 'ingestion')}
						>
							<Database class="size-4" />
							Ingestion
						</Button>
						<Button
							variant={activeTab === 'appearance' ? 'secondary' : 'ghost'}
							class="justify-start gap-2 text-sm font-medium"
							onclick={() => (activeTab = 'appearance')}
						>
							<Palette class="size-4" />
							Appearance
						</Button>
						<Button
							variant={activeTab === 'providers' ? 'secondary' : 'ghost'}
							class="justify-start gap-2 text-sm font-medium"
							onclick={() => (activeTab = 'providers')}
						>
							<Globe class="size-4" />
							Provider defaults
						</Button>
					</nav>
				</aside>

				<!-- Main content area -->
				<div class="flex-1 lg:max-w-3xl">
					<form
						class="flex flex-col gap-6"
						aria-label="Application settings form"
						novalidate
						onsubmit={(event) => {
							event.preventDefault();
							void save();
						}}
					>
						{#if activeTab === 'ingestion'}
							<div class="space-y-6">
								<div>
									<h2 class="text-xl font-semibold tracking-tight">
										Ingestion defaults
									</h2>
									<p class="text-sm text-muted-foreground">
										Control how DeepRef retrieves and retries evidence across
										the application.
									</p>
								</div>
								<Separator />

								<Field.FieldGroup class="grid gap-5 sm:grid-cols-2">
									<Field.Field
										data-invalid={Boolean(validationErrors.crossref_mailto)}
									>
										<Field.FieldLabel for="mailto"
											>Crossref mailto</Field.FieldLabel
										>
										<Input
											id="mailto"
											value={draft.crossref_mailto}
											aria-required="true"
											aria-invalid={validationErrors.crossref_mailto
												? 'true'
												: undefined}
											aria-describedby={validationErrors.crossref_mailto
												? 'mailto-description mailto-error'
												: 'mailto-description'}
											disabled={updateSettings.isPending}
											oninput={(event) =>
												setDraft(
													'crossref_mailto',
													event.currentTarget.value
												)}
											placeholder="research@example.org"
										/>
										<Field.FieldDescription id="mailto-description">
											Used for the Crossref polite pool and request
											User-Agent.
										</Field.FieldDescription>
										{#if validationErrors.crossref_mailto}
											<Field.FieldError id="mailto-error"
												>{validationErrors.crossref_mailto}</Field.FieldError
											>
										{/if}
									</Field.Field>

									<Field.Field
										data-invalid={Boolean(validationErrors.default_max_depth)}
									>
										<Field.FieldLabel for="depth"
											>Default max depth</Field.FieldLabel
										>
										<Input
											id="depth"
											type="number"
											min="0"
											step="1"
											inputmode="numeric"
											value={draft.default_max_depth}
											aria-invalid={validationErrors.default_max_depth
												? 'true'
												: undefined}
											aria-describedby={validationErrors.default_max_depth
												? 'depth-description depth-error'
												: 'depth-description'}
											disabled={updateSettings.isPending}
											oninput={(event) =>
												setDraft(
													'default_max_depth',
													event.currentTarget.value
												)}
										/>
										<Field.FieldDescription id="depth-description">
											Maximum citation depth for a new ingestion.
										</Field.FieldDescription>
										{#if validationErrors.default_max_depth}
											<Field.FieldError id="depth-error"
												>{validationErrors.default_max_depth}</Field.FieldError
											>
										{/if}
									</Field.Field>

									<Field.Field
										data-invalid={Boolean(validationErrors.max_concurrency)}
									>
										<Field.FieldLabel for="concurrency"
											>Max concurrency</Field.FieldLabel
										>
										<Input
											id="concurrency"
											type="number"
											min="1"
											step="1"
											inputmode="numeric"
											value={draft.max_concurrency}
											aria-invalid={validationErrors.max_concurrency
												? 'true'
												: undefined}
											aria-describedby={validationErrors.max_concurrency
												? 'concurrency-description concurrency-error'
												: 'concurrency-description'}
											disabled={updateSettings.isPending}
											oninput={(event) =>
												setDraft(
													'max_concurrency',
													event.currentTarget.value
												)}
										/>
										<Field.FieldDescription id="concurrency-description">
											Number of provider requests that may run concurrently.
										</Field.FieldDescription>
										{#if validationErrors.max_concurrency}
											<Field.FieldError id="concurrency-error"
												>{validationErrors.max_concurrency}</Field.FieldError
											>
										{/if}
									</Field.Field>

									<Field.Field
										data-invalid={Boolean(
											validationErrors.rate_limit_per_second
										)}
									>
										<Field.FieldLabel for="rate"
											>Rate limit per second</Field.FieldLabel
										>
										<Input
											id="rate"
											type="number"
											min="1"
											step="1"
											inputmode="numeric"
											value={draft.rate_limit_per_second}
											aria-invalid={validationErrors.rate_limit_per_second
												? 'true'
												: undefined}
											aria-describedby={validationErrors.rate_limit_per_second
												? 'rate-description rate-error'
												: 'rate-description'}
											disabled={updateSettings.isPending}
											oninput={(event) =>
												setDraft(
													'rate_limit_per_second',
													event.currentTarget.value
												)}
										/>
										<Field.FieldDescription id="rate-description">
											Provider request budget per second, shared globally.
										</Field.FieldDescription>
										{#if validationErrors.rate_limit_per_second}
											<Field.FieldError id="rate-error"
												>{validationErrors.rate_limit_per_second}</Field.FieldError
											>
										{/if}
									</Field.Field>

									<Field.Field
										data-invalid={Boolean(validationErrors.retry_attempts)}
									>
										<Field.FieldLabel for="retry"
											>Retry attempts</Field.FieldLabel
										>
										<Input
											id="retry"
											type="number"
											min="1"
											step="1"
											inputmode="numeric"
											value={draft.retry_attempts}
											aria-invalid={validationErrors.retry_attempts
												? 'true'
												: undefined}
											aria-describedby={validationErrors.retry_attempts
												? 'retry-description retry-error'
												: 'retry-description'}
											disabled={updateSettings.isPending}
											oninput={(event) =>
												setDraft(
													'retry_attempts',
													event.currentTarget.value
												)}
										/>
										<Field.FieldDescription id="retry-description">
											Attempts after a provider request fails.
										</Field.FieldDescription>
										{#if validationErrors.retry_attempts}
											<Field.FieldError id="retry-error"
												>{validationErrors.retry_attempts}</Field.FieldError
											>
										{/if}
									</Field.Field>
								</Field.FieldGroup>

								<div
									class="flex flex-wrap items-center justify-between gap-4 border-t pt-4"
								>
									<div
										class="flex min-w-0 items-center gap-2 text-sm text-muted-foreground"
										data-testid="settings-save-status"
										role={saveStatus === 'error' ? 'alert' : 'status'}
										aria-live="polite"
									>
										<span
											class="inline-block size-2 rounded-full {saveStatus ===
											'saved'
												? 'bg-emerald-500'
												: saveStatus === 'dirty'
													? 'bg-amber-500'
													: saveStatus === 'error'
														? 'bg-destructive'
														: 'bg-muted-foreground/40'}"
										></span>
										<span>{saveStatusLabel}</span>
									</div>
									<Button
										type="submit"
										disabled={updateSettings.isPending ||
											!isDirty ||
											hasValidationErrors}
										data-testid="save-settings"
									>
										{#if updateSettings.isPending}
											<Spinner data-icon="inline-start" />
											Saving settings…
										{:else}
											Save settings
										{/if}
									</Button>
								</div>
							</div>
						{:else if activeTab === 'appearance'}
							<div class="space-y-6">
								<div>
									<h2 class="text-xl font-semibold tracking-tight">Appearance</h2>
									<p class="text-sm text-muted-foreground">
										Customize how DeepRef looks and feels on your device.
									</p>
								</div>
								<Separator />
								<p class="text-sm text-muted-foreground">
									Theme preferences are managed globally via the header toggle.
								</p>
							</div>
						{:else if activeTab === 'providers'}
							<div class="space-y-6">
								<div>
									<h2 class="text-xl font-semibold tracking-tight">
										Provider defaults
									</h2>
									<p class="text-sm text-muted-foreground">
										Default AI model providers and fallback order.
									</p>
								</div>
								<Separator />
								<p class="text-sm text-muted-foreground">
									Provider configuration is configured in project assistant
									settings.
								</p>
							</div>
						{/if}
					</form>
				</div>
			</div>
		{/if}
	</div>
</div>
