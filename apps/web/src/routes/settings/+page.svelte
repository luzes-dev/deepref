<script lang="ts">
	import type { SettingsDto, UpdateSettings } from '$lib/api/generated/models';
	import { createGetSettings, createUpdateSettings } from '$lib/api/generated/settings/settings';
	import { PageHeader, PageToolbar, StatePanel, Surface } from '$lib/components/layout';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Switch } from '$lib/components/ui/switch';
	import { ThemeSelector } from '$lib/components/ui/theme-selector';

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
		} catch {
			// The mutation error is rendered in the save status and alert.
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

<div class="min-h-svh bg-background" data-testid="settings-page">
	<div class="mx-auto flex w-full max-w-[1200px] flex-col gap-6 p-4 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Application controls"
			title="Settings"
			description="Set the defaults that shape every evidence workspace. Project-specific choices remain in each project workflow."
		/>

		<PageToolbar label="Application scope">
			<Badge variant="secondary">Application scope</Badge>
			<span class="text-sm text-muted-foreground"
				>Changes apply to new and active projects.</span
			>
		</PageToolbar>

		{#if isLoading}
			<div data-testid="settings-loading">
				<Surface tone="subtle" class="p-4 sm:p-6">
					<StatePanel
						state="loading"
						title="Loading application settings"
						description="Retrieving the current provider and ingestion defaults."
					/>
				</Surface>
			</div>
		{:else if loadError}
			<div data-testid="settings-load-error">
				<Surface tone="subtle" class="p-4 sm:p-6">
					<StatePanel state="error" title="Settings unavailable" description={loadError}>
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
		{:else if baseline}
			<form
				class="flex flex-col gap-6"
				aria-label="Application settings form"
				novalidate
				onsubmit={(event) => {
					event.preventDefault();
					void save();
				}}
			>
				<div class="grid gap-6 lg:grid-cols-[minmax(0,1.45fr)_minmax(18rem,0.85fr)]">
					<Card.Root>
						<Card.Header>
							<Card.Title role="heading" aria-level={2}>Ingestion defaults</Card.Title
							>
							<Card.Description>
								Control how DeepRef retrieves and retries evidence across the
								application.
							</Card.Description>
						</Card.Header>
						<Card.Content>
							<Field.FieldGroup class="grid gap-5 sm:grid-cols-2">
								<Field.Field
									data-invalid={Boolean(validationErrors.crossref_mailto)}
								>
									<Field.FieldLabel for="mailto">Crossref mailto</Field.FieldLabel
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
											setDraft('crossref_mailto', event.currentTarget.value)}
										placeholder="research@example.org"
									/>
									<Field.FieldDescription id="mailto-description">
										Used for the Crossref polite pool and request User-Agent.
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
											setDraft('max_concurrency', event.currentTarget.value)}
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
									data-invalid={Boolean(validationErrors.rate_limit_per_second)}
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
									<Field.FieldLabel for="retry">Retry attempts</Field.FieldLabel>
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
											setDraft('retry_attempts', event.currentTarget.value)}
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
						</Card.Content>
					</Card.Root>

					<div class="flex flex-col gap-6">
						<Card.Root>
							<Card.Header>
								<Card.Title role="heading" aria-level={2}>Appearance</Card.Title>
								<Card.Description
									>Choose how the evidence atelier appears on this device.</Card.Description
								>
							</Card.Header>
							<Card.Content>
								<Field.FieldGroup>
									<Field.Field orientation="responsive">
										<Field.FieldContent>
											<Field.FieldTitle id="theme-label"
												>Theme</Field.FieldTitle
											>
											<Field.FieldDescription>
												Use system, light, or dark mode for this browser.
											</Field.FieldDescription>
										</Field.FieldContent>
										<ThemeSelector />
									</Field.Field>
								</Field.FieldGroup>
							</Card.Content>
						</Card.Root>

						<Card.Root>
							<Card.Header>
								<Card.Title role="heading" aria-level={2}
									>Provider defaults</Card.Title
								>
								<Card.Description>
									Read-only providers currently used by new evidence requests.
								</Card.Description>
							</Card.Header>
							<Card.Content>
								<Field.FieldGroup>
									<Field.Field orientation="responsive" data-disabled>
										<Field.FieldContent>
											<Field.FieldLabel for="metadata-provider"
												>Metadata provider</Field.FieldLabel
											>
											<Field.FieldDescription
												>Source for titles and publication metadata.</Field.FieldDescription
											>
										</Field.FieldContent>
										<Input
											id="metadata-provider"
											value={settingsQueryResult.data?.data
												.metadata_provider ?? 'crossref'}
											readonly
											disabled
										/>
									</Field.Field>
									<Field.Field orientation="responsive" data-disabled>
										<Field.FieldContent>
											<Field.FieldLabel for="citation-provider"
												>Citation provider</Field.FieldLabel
											>
											<Field.FieldDescription
												>Source for citation and reference links.</Field.FieldDescription
											>
										</Field.FieldContent>
										<Input
											id="citation-provider"
											value={settingsQueryResult.data?.data
												.citation_provider ?? 'crossref'}
											readonly
											disabled
										/>
									</Field.Field>
								</Field.FieldGroup>
							</Card.Content>
						</Card.Root>
					</div>
				</div>

				{#if updateSettings.error}
					<Alert.Root variant="destructive" data-testid="settings-save-error">
						<Alert.Title>Could not save settings</Alert.Title>
						<Alert.Description>
							{updateSettings.error.message ||
								'The application settings request failed.'}
						</Alert.Description>
					</Alert.Root>
				{/if}

				<Card.Root size="sm">
					<Card.Footer class="flex-wrap justify-between gap-3 border-t">
						<div
							class="flex min-w-0 items-center gap-2 text-sm text-muted-foreground"
							data-testid="settings-save-status"
							role={saveStatus === 'error' ? 'alert' : 'status'}
							aria-live="polite"
						>
							<Switch
								checked={saveStatus === 'saved'}
								disabled
								aria-label="Settings saved"
							/>
							<span>{saveStatusLabel}</span>
						</div>
						<Button
							type="submit"
							disabled={updateSettings.isPending || !isDirty || hasValidationErrors}
							data-testid="save-settings"
						>
							{#if updateSettings.isPending}
								<Spinner data-icon="inline-start" />
								Saving settings…
							{:else}
								Save settings
							{/if}
						</Button>
					</Card.Footer>
				</Card.Root>
			</form>
		{/if}
	</div>
</div>
