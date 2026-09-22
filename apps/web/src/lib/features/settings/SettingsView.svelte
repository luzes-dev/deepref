<script lang="ts">
	import { Database, Globe, Maximize2, Minimize2, Palette, Search, X } from '@lucide/svelte';
	import type { LucideIcon } from '@lucide/svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import type { SettingsDto, UpdateSettings } from '$lib/api/generated/models';
	import {
		createGetSettings,
		createUpdateSettings,
		getGetSettingsQueryKey
	} from '$lib/api/generated/settings/settings';
	import { StatePanel, Surface } from '@deepref/ui/layout';
	import * as NumberField from '@deepref/ui/number-field';
	import { notifyError } from '$lib/features/notifications/toast';
	import { Button } from '@deepref/ui/button';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import { Separator } from '@deepref/ui/separator';
	import { Spinner } from '@deepref/ui/spinner';
	import * as Select from '@deepref/ui/select';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import { Debounced, onCleanup, watch } from 'runed';
	import { enqueueSettingsSave } from './settings-save-queue';
	import { cn } from '$lib/utils';

	type Presentation = 'page' | 'modal';
	export type SettingsViewProps = {
		presentation?: Presentation;
		onclose?: () => void;
		onexpand?: () => void;
		oncollapse?: () => void;
	};
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
	type SaveStatus = 'ready' | 'dirty' | 'saving' | 'saved' | 'error' | 'invalid';
	type SettingsSection = 'ingestion' | 'appearance' | 'providers';
	type ThemeMode = 'light' | 'dark' | 'system';

	let { presentation = 'page', onclose, onexpand, oncollapse }: SettingsViewProps = $props();

	const numericFields = [
		{
			key: 'default_max_depth',
			id: 'depth',
			label: 'Default max depth',
			minimum: 0,
			description: 'Maximum citation depth for a new ingestion.'
		},
		{
			key: 'max_concurrency',
			id: 'concurrency',
			label: 'Max concurrency',
			minimum: 1,
			description: 'Number of provider requests that may run concurrently.'
		},
		{
			key: 'rate_limit_per_second',
			id: 'rate',
			label: 'Rate limit per second',
			minimum: 1,
			description: 'Provider request budget per second, shared globally.'
		},
		{
			key: 'retry_attempts',
			id: 'retry',
			label: 'Retry attempts',
			minimum: 1,
			description: 'Attempts after a provider request fails.'
		}
	] as const satisfies readonly {
		key: NumericSettingKey;
		id: string;
		label: string;
		minimum: number;
		description: string;
	}[];
	const numericFieldKeys: readonly NumericSettingKey[] = numericFields.map((field) => field.key);
	const sectionItems: readonly {
		id: SettingsSection;
		label: string;
		description: string;
		searchText: string;
		icon: LucideIcon;
	}[] = [
		{
			id: 'ingestion',
			label: 'Ingestion',
			description: 'Provider and ingestion defaults',
			searchText: [
				'Crossref mailto used for the Crossref polite pool and request User-Agent.',
				...numericFields.flatMap((field) => [field.label, field.description])
			].join(' '),
			icon: Database
		},
		{
			id: 'appearance',
			label: 'Appearance',
			description: 'Customize the application look and feel',
			searchText:
				'Choose whether DeepRef follows your system preference or uses a fixed theme. Theme preferences Light Dark System.',
			icon: Palette
		},
		{
			id: 'providers',
			label: 'Provider defaults',
			description: 'Default provider configuration',
			searchText:
				'Default AI model providers and fallback order. Provider configuration is configured in project assistant settings.',
			icon: Globe
		}
	];

	const settingsQueryResult = createGetSettings();
	const updateSettings = createUpdateSettings();
	const queryClient = useQueryClient();

	let activeTab = $state<SettingsSection>('ingestion');
	let searchTerm = $state('');
	let submittedSettings = $state<SettingsDto | null>(null);
	let edits = $state<Partial<DraftSettings>>({});
	let validationErrors = $state<ValidationErrors>({});
	let userHasEdited = $state(false);
	let savePhase = $state<'idle' | 'saving' | 'saved' | 'error'>('idle');
	let saveError = $state('');
	let editRevision = 0;
	let scheduledRevision = 0;
	let activeSaveCount = $state(0);
	let destroyed = false;

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
		hasValidationErrors
			? 'invalid'
			: savePhase === 'saving' || activeSaveCount > 0
				? 'saving'
				: savePhase === 'error'
					? 'error'
					: savePhase === 'saved'
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
				error: 'Save failed',
				invalid: 'Fix validation errors'
			} satisfies Record<SaveStatus, string>
		)[saveStatus]
	);
	const debouncedDraft = new Debounced(() => draft, 450);
	const visibleSections = $derived(
		sectionItems.filter((section) => {
			const query = searchTerm.trim().toLowerCase();
			return (
				!query ||
				section.label.toLowerCase().includes(query) ||
				section.description.toLowerCase().includes(query) ||
				section.searchText.toLowerCase().includes(query)
			);
		})
	);
	const activeSection = $derived(
		sectionItems.find((section) => section.id === activeTab) ?? sectionItems[0]
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
		const field = numericFields.find((item) => item.key === key);
		const value = Number(rawValue);
		if (!field || rawValue.trim() === '' || !Number.isInteger(value) || value < field.minimum) {
			return field
				? `${field.label} must be an integer of at least ${field.minimum}.`
				: 'This setting must be a valid integer.';
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
		userHasEdited = true;
		editRevision += 1;
		edits = { ...edits, [key]: value };
		const error = fieldError(key, value);
		const nextErrors = { ...validationErrors };
		if (error) nextErrors[key] = error;
		else delete nextErrors[key];
		validationErrors = nextErrors;
		saveError = '';
		if (activeSaveCount === 0) savePhase = 'idle';
	}

	function numericDraftValue(key: NumericSettingKey): number | undefined {
		const value = Number(draft[key]);
		return draft[key].trim() === '' || !Number.isFinite(value) ? undefined : value;
	}

	function setNumericDraft(key: NumericSettingKey, value: number | undefined): void {
		setDraft(key, value === undefined || !Number.isFinite(value) ? '' : String(value));
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

	let latestSavePromise: Promise<void> = Promise.resolve();

	function mergeWithCurrentSettings(
		data: UpdateSettings,
		changedKeys: readonly DraftKey[]
	): UpdateSettings {
		const current = queryClient.getQueryData<{ data: SettingsDto }>(
			getGetSettingsQueryKey()
		)?.data;
		if (!current) return data;

		const changed = new Set(changedKeys);
		return {
			crossref_mailto: changed.has('crossref_mailto')
				? data.crossref_mailto
				: current.crossref_mailto,
			default_max_depth: changed.has('default_max_depth')
				? data.default_max_depth
				: current.default_max_depth,
			max_concurrency: changed.has('max_concurrency')
				? data.max_concurrency
				: current.max_concurrency,
			rate_limit_per_second: changed.has('rate_limit_per_second')
				? data.rate_limit_per_second
				: current.rate_limit_per_second,
			retry_attempts: changed.has('retry_attempts')
				? data.retry_attempts
				: current.retry_attempts
		};
	}

	async function saveDraft(
		snapshot: DraftSettings,
		revision: number,
		changedKeys: readonly DraftKey[]
	): Promise<void> {
		const result = validateDraft(snapshot);
		if (!result.ok) {
			if (revision === editRevision) validationErrors = result.errors;
			return;
		}

		activeSaveCount += 1;
		savePhase = 'saving';
		try {
			const response = await enqueueSettingsSave(queryClient, async () => {
				const response = await updateSettings.mutateAsync({
					data: mergeWithCurrentSettings(result.data, changedKeys)
				});
				// Publish before releasing the shared queue so a newly mounted
				// settings view starts its mutation from this response.
				queryClient.setQueryData(getGetSettingsQueryKey(), response);
				return response;
			});
			if (!destroyed) submittedSettings = response.data;
			if (!destroyed && revision === editRevision) {
				edits = {};
				validationErrors = {};
				saveError = '';
				savePhase = 'saved';
			}
		} catch (error) {
			if (!destroyed && revision === editRevision) {
				saveError = error instanceof Error ? error.message : 'The settings request failed.';
				savePhase = 'error';
			}
			notifyError(
				'Could not save settings',
				error,
				'The application settings request failed.'
			);
		} finally {
			if (!destroyed) {
				activeSaveCount -= 1;
				if (activeSaveCount === 0 && savePhase === 'saving') savePhase = 'idle';
			}
		}
	}

	function scheduleSave(
		snapshot: DraftSettings,
		revision: number,
		changedKeys: readonly DraftKey[] = Object.keys(edits) as DraftKey[]
	): void {
		const promise = saveDraft(snapshot, revision, [...changedKeys]);
		latestSavePromise = promise.catch(() => undefined);
	}

	watch(
		() => debouncedDraft.current,
		(snapshot) => {
			if (!userHasEdited || !baseline || editRevision <= scheduledRevision) return;
			if (!sameDraft(snapshot, draft)) return;

			scheduledRevision = editRevision;
			scheduleSave({ ...snapshot }, editRevision);
		}
	);

	async function flushPendingSave(): Promise<void> {
		if (userHasEdited && baseline) {
			const revision = editRevision;
			if (revision > scheduledRevision) {
				scheduledRevision = revision;
				scheduleSave({ ...draft }, revision);
			}
		}
		await latestSavePromise;
	}

	async function handleClose(): Promise<void> {
		await flushPendingSave();
		onclose?.();
	}

	async function handleExpand(): Promise<void> {
		await flushPendingSave();
		onexpand?.();
	}

	async function handleCollapse(): Promise<void> {
		await flushPendingSave();
		oncollapse?.();
	}

	function setThemeMode(value: string | undefined): void {
		if (value === 'light' || value === 'dark' || value === 'system') setMode(value);
	}

	function themeModeLabel(value: ThemeMode): string {
		return value === 'light' ? 'Light' : value === 'dark' ? 'Dark' : 'System';
	}

	onCleanup(() => {
		destroyed = true;
		debouncedDraft.cancel();
		void flushPendingSave();
	});
</script>

<svelte:head>
	<title>Settings · DeepRef</title>
	<meta
		name="description"
		content="Configure application-wide ingestion and evidence provider defaults."
	/>
</svelte:head>

<div
	class={cn(
		'flex min-h-0 min-w-0 flex-col bg-background text-foreground',
		presentation === 'modal' ? 'h-full w-full overflow-hidden' : 'min-h-svh overflow-auto'
	)}
	data-testid={presentation === 'page' ? 'settings-page' : 'settings-view'}
	data-presentation={presentation}
>
	{#if presentation === 'modal'}
		<div
			class="grid min-h-0 flex-1 grid-cols-[13.125rem_minmax(0,1fr)] max-[640px]:grid-cols-1 max-[640px]:grid-rows-[auto_minmax(0,1fr)]"
		>
			<aside
				class="min-h-0 overflow-y-auto border-r border-border-subtle bg-muted/20 max-[640px]:max-h-44 max-[640px]:border-r-0 max-[640px]:border-b"
			>
				<div
					class="flex flex-col gap-3 p-2.5 max-[640px]:flex-row max-[640px]:items-center"
				>
					<label class="relative block max-[640px]:min-w-0 max-[640px]:flex-1">
						<span class="sr-only">Search settings</span>
						<Search
							class="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground"
							aria-hidden="true"
						/>
						<Input
							value={searchTerm}
							oninput={(event) => (searchTerm = event.currentTarget.value)}
							placeholder="Search settings"
							aria-label="Search settings"
							class="h-9 rounded-full pl-9 text-xs"
						/>
					</label>
				</div>

				<nav
					class="flex flex-col gap-0.5 px-2 pb-4 max-[640px]:w-full max-[640px]:min-w-0 max-[640px]:flex-row max-[640px]:overflow-x-auto"
					aria-label="Settings navigation"
				>
					{#each visibleSections as section (section.id)}
						{@const Icon = section.icon}
						<Button
							variant={activeTab === section.id ? 'secondary' : 'ghost'}
							class="h-9 w-full justify-start gap-2 px-2.5 text-left text-sm font-normal max-[640px]:w-auto"
							aria-current={activeTab === section.id ? 'page' : undefined}
							onclick={() => (activeTab = section.id)}
						>
							<Icon aria-hidden="true" />
							{section.label}
						</Button>
					{/each}
					{#if visibleSections.length === 0}
						<p class="px-2.5 py-3 text-xs text-muted-foreground">
							No matching sections.
						</p>
					{/if}
				</nav>
			</aside>

			<section class="flex min-h-0 min-w-0 flex-col" aria-labelledby="settings-title">
				<header
					class="flex min-h-14 shrink-0 items-center justify-between gap-3 border-b border-border-subtle px-4"
				>
					<h1 id="settings-title" class="text-base font-medium">{activeSection.label}</h1>
					<div class="flex items-center gap-1">
						{#if onexpand}
							<Button
								variant="ghost"
								size="icon-sm"
								class="rounded-md"
								onclick={handleExpand}
								title="Open full settings"
								aria-label="Open full settings"
							>
								<Maximize2 aria-hidden="true" />
							</Button>
						{/if}
						{#if onclose}
							<Button
								variant="ghost"
								size="icon-sm"
								class="rounded-md"
								onclick={handleClose}
								aria-label="Close settings"
							>
								<X aria-hidden="true" />
							</Button>
						{/if}
					</div>
				</header>
				<div class="min-h-0 flex-1 overflow-y-auto p-4">
					{#if isLoading}
						<div data-testid="settings-loading">
							<StatePanel
								state="loading"
								title="Loading application settings"
								description="Retrieving the current provider and ingestion defaults."
							/>
						</div>
					{:else if loadError}
						<div data-testid="settings-load-error">
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
						</div>
					{:else if baseline}
						{@render settingsBody()}
					{/if}
				</div>
			</section>
		</div>
	{:else}
		<div
			class="mx-auto flex w-full max-w-[100rem] flex-col gap-8 px-6 py-8 sm:px-10 sm:py-10 lg:px-12 lg:py-12"
		>
			<header class="flex items-start justify-between gap-6">
				<div class="flex flex-col gap-1">
					<h1 class="text-3xl font-semibold tracking-tight">Settings</h1>
					<p class="text-base text-muted-foreground">
						Manage application settings and review ingestion preferences.
					</p>
				</div>
				{#if oncollapse}
					<Button
						variant="ghost"
						size="icon-sm"
						class="rounded-md"
						onclick={handleCollapse}
						title="Collapse settings"
						aria-label="Collapse settings"
					>
						<Minimize2 aria-hidden="true" />
					</Button>
				{/if}
			</header>
			<Separator />

			{#if isLoading}
				<div data-testid="settings-loading">
					<div
						class="grid gap-8 lg:grid-cols-[minmax(0,19rem)_minmax(0,52rem)] lg:gap-10"
					>
						<aside>
							<nav
								class="flex flex-row gap-1 overflow-x-auto lg:flex-col"
								aria-label="Settings navigation"
							>
								<Button variant="secondary" class="justify-start text-sm" disabled>
									<Database data-icon="inline-start" aria-hidden="true" />
									Ingestion
								</Button>
							</nav>
						</aside>
						<div class="min-w-0">
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
					<div
						class="grid gap-8 lg:grid-cols-[minmax(0,19rem)_minmax(0,52rem)] lg:gap-10"
					>
						<div class="min-w-0">
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
				<div class="grid gap-8 lg:grid-cols-[minmax(0,19rem)_minmax(0,52rem)] lg:gap-10">
					<aside>
						<label class="relative mb-3 block">
							<span class="sr-only">Search settings</span>
							<Search
								class="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground"
								aria-hidden="true"
							/>
							<Input
								value={searchTerm}
								oninput={(event) => (searchTerm = event.currentTarget.value)}
								placeholder="Search settings"
								aria-label="Search settings"
								class="h-9 rounded-full pl-9 text-xs"
							/>
						</label>
						<nav
							class="flex flex-row gap-1 overflow-x-auto lg:flex-col"
							aria-label="Settings navigation"
						>
							{#each visibleSections as section (section.id)}
								{@const Icon = section.icon}
								<Button
									variant={activeTab === section.id ? 'secondary' : 'ghost'}
									class="justify-start gap-2 text-sm font-medium"
									aria-current={activeTab === section.id ? 'page' : undefined}
									onclick={() => (activeTab = section.id)}
								>
									<Icon aria-hidden="true" />
									{section.label}
								</Button>
							{/each}
							{#if visibleSections.length === 0}
								<p class="px-2 py-3 text-xs text-muted-foreground">
									No matching sections.
								</p>
							{/if}
						</nav>
					</aside>
					<div class="max-w-3xl min-w-0">
						{@render settingsBody()}
					</div>
				</div>
			{/if}
		</div>
	{/if}
</div>

{#snippet settingsBody()}
	<form
		class="flex flex-col gap-6"
		aria-label="Application settings form"
		novalidate
		onsubmit={(event) => event.preventDefault()}
	>
		{#if activeTab === 'ingestion'}
			<div class="flex flex-col gap-6">
				{#if presentation === 'page'}
					<div class="flex flex-col gap-1">
						<h2 class="text-xl font-semibold tracking-tight">Ingestion defaults</h2>
						<p class="text-sm text-muted-foreground">
							Control how DeepRef retrieves and retries evidence across the
							application.
						</p>
					</div>
					<Separator />
				{/if}

				<Field.FieldGroup
					class={presentation === 'modal'
						? 'gap-0 divide-y divide-border-subtle'
						: undefined}
				>
					<Field.Field
						data-invalid={Boolean(validationErrors.crossref_mailto)}
						class={presentation === 'modal'
							? 'grid grid-cols-[minmax(0,1fr)_minmax(8rem,14rem)] items-center gap-x-5 gap-y-1 py-3 max-[640px]:flex max-[640px]:flex-col max-[640px]:items-stretch max-[640px]:gap-y-2'
							: undefined}
					>
						<Field.FieldLabel
							for="mailto"
							class={presentation === 'modal'
								? 'min-[641px]:col-start-1 min-[641px]:row-start-1'
								: undefined}
						>
							Crossref mailto
						</Field.FieldLabel>
						<Input
							id="mailto"
							value={draft.crossref_mailto}
							aria-required="true"
							aria-invalid={validationErrors.crossref_mailto ? 'true' : undefined}
							aria-describedby={validationErrors.crossref_mailto
								? 'mailto-description mailto-error'
								: 'mailto-description'}
							oninput={(event) =>
								setDraft('crossref_mailto', event.currentTarget.value)}
							placeholder="research@example.org"
							class={presentation === 'modal'
								? 'min-[641px]:col-start-2 min-[641px]:row-span-2 min-[641px]:row-start-1'
								: undefined}
						/>
						<Field.FieldDescription
							id="mailto-description"
							class={presentation === 'modal'
								? 'min-[641px]:col-start-1 min-[641px]:row-start-2'
								: undefined}
						>
							Used for the Crossref polite pool and request User-Agent.
						</Field.FieldDescription>
						{#if validationErrors.crossref_mailto}
							<Field.FieldError
								id="mailto-error"
								class={presentation === 'modal'
									? 'min-[641px]:col-start-1 min-[641px]:row-start-3'
									: undefined}
								>{validationErrors.crossref_mailto}</Field.FieldError
							>
						{/if}
					</Field.Field>

					{#each numericFields as field (field.key)}
						<Field.Field
							data-invalid={Boolean(validationErrors[field.key])}
							class={presentation === 'modal'
								? 'grid grid-cols-[minmax(0,1fr)_minmax(8rem,14rem)] items-center gap-x-5 gap-y-1 py-3 max-[640px]:flex max-[640px]:flex-col max-[640px]:items-stretch max-[640px]:gap-y-2'
								: undefined}
						>
							<Field.FieldLabel
								for={field.id}
								class={presentation === 'modal'
									? 'min-[641px]:col-start-1 min-[641px]:row-start-1'
									: undefined}
							>
								{field.label}
							</Field.FieldLabel>
							<NumberField.Root
								bind:value={
									() => numericDraftValue(field.key),
									(value) => setNumericDraft(field.key, value)
								}
								min={field.minimum}
								step={1}
							>
								<NumberField.Group
									class={presentation === 'modal'
										? 'min-[641px]:col-start-2 min-[641px]:row-span-2 min-[641px]:row-start-1'
										: undefined}
								>
									<NumberField.Decrement />
									<NumberField.Input
										id={field.id}
										inputmode="numeric"
										aria-label={field.label}
										aria-invalid={validationErrors[field.key]
											? 'true'
											: undefined}
										aria-describedby={validationErrors[field.key]
											? `${field.id}-description ${field.id}-error`
											: `${field.id}-description`}
									/>
									<NumberField.Increment />
								</NumberField.Group>
							</NumberField.Root>
							<Field.FieldDescription
								id={`${field.id}-description`}
								class={presentation === 'modal'
									? 'min-[641px]:col-start-1 min-[641px]:row-start-2'
									: undefined}
							>
								{field.description}
							</Field.FieldDescription>
							{#if validationErrors[field.key]}
								<Field.FieldError
									id={`${field.id}-error`}
									class={presentation === 'modal'
										? 'min-[641px]:col-start-1 min-[641px]:row-start-3'
										: undefined}
								>
									{validationErrors[field.key]}
								</Field.FieldError>
							{/if}
						</Field.Field>
					{/each}
				</Field.FieldGroup>

				<div
					class="flex flex-wrap items-center justify-between gap-4 border-t border-border-subtle pt-4"
				>
					<div
						class="flex min-w-0 items-center gap-2 text-sm text-muted-foreground"
						data-testid="settings-save-status"
						role={saveStatus === 'error' ? 'alert' : 'status'}
						aria-live="polite"
					>
						{#if saveStatus === 'saving'}
							<Spinner class="size-3.5" aria-hidden="true" />
						{:else}
							<span
								class={cn(
									'inline-block size-2 rounded-full',
									saveStatus === 'saved'
										? 'bg-success'
										: saveStatus === 'dirty'
											? 'bg-warning'
											: saveStatus === 'error' || saveStatus === 'invalid'
												? 'bg-destructive'
												: 'bg-muted-foreground/40'
								)}
							></span>
						{/if}
						<span>{saveStatusLabel}</span>
						{#if saveError && saveStatus === 'error'}
							<span class="sr-only">{saveError}</span>
						{/if}
					</div>
				</div>
			</div>
		{:else if activeTab === 'appearance'}
			<div class="flex flex-col gap-6">
				{#if presentation === 'page'}
					<div class="flex flex-col gap-1">
						<h2 class="text-xl font-semibold tracking-tight">Appearance</h2>
						<p class="text-sm text-muted-foreground">
							Customize how DeepRef looks and feels on your device.
						</p>
					</div>
					<Separator />
				{/if}
				<div class="flex flex-wrap items-center justify-between gap-4">
					<p class="text-sm leading-relaxed text-muted-foreground">
						Choose whether DeepRef follows your system preference or uses a fixed theme.
					</p>
					<Select.Root
						type="single"
						bind:value={() => userPrefersMode.current, (value) => setThemeMode(value)}
					>
						<Select.Trigger aria-label="Theme" class="w-32">
							{themeModeLabel(userPrefersMode.current as ThemeMode)}
						</Select.Trigger>
						<Select.Content>
							<Select.Item value="light" label="Light" />
							<Select.Item value="dark" label="Dark" />
							<Select.Item value="system" label="System" />
						</Select.Content>
					</Select.Root>
				</div>
			</div>
		{:else}
			<div class="flex flex-col gap-6">
				{#if presentation === 'page'}
					<div class="flex flex-col gap-1">
						<h2 class="text-xl font-semibold tracking-tight">Provider defaults</h2>
						<p class="text-sm text-muted-foreground">
							Default AI model providers and fallback order.
						</p>
					</div>
					<Separator />
				{/if}
				<p class="text-sm leading-relaxed text-muted-foreground">
					Provider configuration is configured in project assistant settings.
				</p>
			</div>
		{/if}
	</form>
{/snippet}
