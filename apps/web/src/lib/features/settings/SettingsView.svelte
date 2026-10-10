<script lang="ts">
	import { Database, Maximize2, Minimize2, Palette, Search, Sparkles, X } from '@lucide/svelte';
	import type { LucideIcon } from '@lucide/svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import type { SettingsDto, UpdateSettings } from '#lib/api/generated/models/index.js';
	import {
		createGetSettings,
		createUpdateSettings,
		getGetSettingsQueryKey
	} from '#lib/api/generated/settings/settings.js';
	import { StatePanel } from '@deepref/ui/layout';
	import * as NumberField from '@deepref/ui/number-field';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import { Button } from '@deepref/ui/button';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import { Spinner } from '@deepref/ui/spinner';
	import * as Select from '@deepref/ui/select';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import { page } from '$app/state';
	import AutonomySettings from '#lib/features/ai-autonomy/components/AutonomySettings.svelte';
	import { Debounced, onCleanup, watch } from 'runed';
	import { enqueueSettingsSave } from './settings-save-queue';
	import { crossrefMailtoError } from './crossref-mailto';

	export type SettingsViewProps = {
		presentation?: 'page' | 'modal';
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
	type SettingsSection = 'ingestion' | 'ai' | 'appearance';
	type ThemeMode = 'light' | 'dark' | 'system';

	let { presentation = 'page', onclose, onexpand, oncollapse }: SettingsViewProps = $props();

	const numericFields = [
		{
			key: 'default_max_depth',
			id: 'depth',
			label: 'Default max depth',
			minimum: 0,
			description:
				'Default citation depth for new imports. Each level can multiply the number of records: depth 1 adds the articles they cite, and depth 2 can add thousands.'
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
			id: 'ai',
			label: 'AI',
			description: 'How far the AI may go in this project',
			searchText:
				'AI autonomy suggest second reviewer act and notify off duplicates screening extraction appraisal monthly budget spend',
			icon: Sparkles
		},
		{
			id: 'appearance',
			label: 'Appearance',
			description: 'Customize the application look and feel',
			searchText:
				'Choose whether DeepRef follows your system preference or uses a fixed theme. Theme preferences Light Dark System.',
			icon: Palette
		}
	];

	const aiProjectId = $derived(page.params.projectId);
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
		if (key === 'crossref_mailto') return crossrefMailtoError(value);
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
		const mailtoError = crossrefMailtoError(value.crossref_mailto);
		if (mailtoError) errors.crossref_mailto = mailtoError;

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
	class="flex h-full min-h-0 min-w-0 flex-col bg-background text-foreground"
	data-testid={presentation === 'page' ? 'settings-page' : 'settings-view'}
	data-presentation={presentation}
>
	<header class="flex shrink-0 items-center justify-between gap-3 px-4 py-3 sm:px-6">
		<div>
			<h1 id="settings-title" class="text-base font-semibold">Settings</h1>
			<p class="mt-1 text-xs text-muted-foreground">
				{activeTab === 'ai' ? 'This project' : 'Application defaults'} · Changes save automatically.
			</p>
		</div>
		<div class="flex items-center gap-1">
			{#if onexpand}<Button
					variant="ghost"
					size="icon-sm"
					onclick={handleExpand}
					aria-label="Open full settings"><Maximize2 /></Button
				>{/if}
			{#if oncollapse}<Button
					variant="ghost"
					size="icon-sm"
					onclick={handleCollapse}
					aria-label="Collapse settings"><Minimize2 /></Button
				>{/if}
			{#if onclose}<Button
					variant="ghost"
					size="icon-sm"
					onclick={handleClose}
					aria-label="Close settings"><X /></Button
				>{/if}
		</div>
	</header>
	<div class="flex shrink-0 flex-wrap items-center justify-between gap-3 px-4 pb-3 sm:px-6">
		{@render settingsNavigation('flex items-center gap-1', '')}
		{@render searchSettings('relative block w-48')}
	</div>
	<section
		class="min-h-0 flex-1 overflow-auto px-4 py-4 sm:px-6"
		aria-labelledby="settings-title"
	>
		<div class="max-w-3xl">
			{#if isLoading}<div data-testid="settings-loading">
					<StatePanel
						state="loading"
						title="Loading application settings"
						description="Retrieving provider and ingestion defaults."
					/>
				</div>
			{:else if loadError}<div data-testid="settings-load-error">
					{@render settingsLoadError()}
				</div>
			{:else if baseline}{@render settingsBody()}{/if}
		</div>
	</section>
</div>

{#snippet searchSettings(labelClass: string)}
	<label class={labelClass}>
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
{/snippet}

{#snippet settingsNavigation(navClass: string, buttonClass: string)}
	<nav class={navClass} aria-label="Settings navigation">
		{#each visibleSections as section (section.id)}
			{@const Icon = section.icon}
			<Button
				variant={activeTab === section.id ? 'secondary' : 'ghost'}
				class={buttonClass}
				aria-current={activeTab === section.id ? 'page' : undefined}
				onclick={() => (activeTab = section.id)}
			>
				<Icon aria-hidden="true" />
				{section.label}
			</Button>
		{/each}
		{#if visibleSections.length === 0}
			<p class="px-2.5 py-3 text-xs text-muted-foreground">No matching sections.</p>
		{/if}
	</nav>
{/snippet}

{#snippet settingsLoadError()}
	<StatePanel state="error" title="Settings unavailable" description={loadError}>
		{#snippet action()}
			<Button variant="outline" onclick={() => void settingsQueryResult.refetch()}>
				Try again
			</Button>
		{/snippet}
	</StatePanel>
{/snippet}

{#snippet numericSetting(field: (typeof numericFields)[number])}
	<Field.Field
		data-invalid={Boolean(validationErrors[field.key])}
		class="grid grid-cols-[minmax(0,1fr)_minmax(8rem,12rem)] items-center gap-x-4 gap-y-2 max-sm:flex max-sm:flex-col max-sm:items-stretch"
	>
		<div class="flex flex-col gap-1">
			<Field.FieldLabel for={field.id}>{field.label}</Field.FieldLabel>
			<Field.FieldDescription id={`${field.id}-description`}
				>{field.description}</Field.FieldDescription
			>
		</div>
		<NumberField.Root
			bind:value={
				() => numericDraftValue(field.key), (value) => setNumericDraft(field.key, value)
			}
			min={field.minimum}
			step={1}
		>
			<NumberField.Group
				><NumberField.Decrement /><NumberField.Input
					id={field.id}
					inputmode="numeric"
					aria-label={field.label}
					aria-invalid={validationErrors[field.key] ? 'true' : undefined}
					aria-describedby={validationErrors[field.key]
						? `${field.id}-description ${field.id}-error`
						: `${field.id}-description`}
				/><NumberField.Increment /></NumberField.Group
			>
		</NumberField.Root>
		{#if validationErrors[field.key]}<Field.FieldError
				id={`${field.id}-error`}
				class="col-span-2">{validationErrors[field.key]}</Field.FieldError
			>{/if}
	</Field.Field>
{/snippet}

{#snippet settingsBody()}
	<form
		class="flex flex-col gap-6"
		aria-label="Application settings form"
		novalidate
		onsubmit={(event) => event.preventDefault()}
	>
		{#if activeTab === 'ingestion'}
			<Field.FieldGroup>
				<Field.Field data-invalid={Boolean(validationErrors.crossref_mailto)}>
					<Field.FieldLabel for="mailto">Crossref contact email</Field.FieldLabel>
					<Input
						id="mailto"
						type="email"
						inputmode="email"
						autocomplete="email"
						value={draft.crossref_mailto}
						aria-required="true"
						aria-invalid={validationErrors.crossref_mailto ? 'true' : undefined}
						aria-describedby={validationErrors.crossref_mailto
							? 'mailto-description mailto-error'
							: 'mailto-description'}
						oninput={(event) => setDraft('crossref_mailto', event.currentTarget.value)}
						placeholder="research@example.org"
					/>
					<Field.FieldDescription id="mailto-description"
						>Identifies requests to Crossref's polite pool.</Field.FieldDescription
					>
					{#if validationErrors.crossref_mailto}<Field.FieldError id="mailto-error"
							>{validationErrors.crossref_mailto}</Field.FieldError
						>{/if}
				</Field.Field>
				{@render numericSetting(numericFields[0])}
			</Field.FieldGroup>
			<details
				open={numericFields
					.slice(1)
					.some(
						(field) =>
							Boolean(validationErrors[field.key]) ||
							(Boolean(searchTerm.trim()) &&
								`${field.label} ${field.description}`
									.toLowerCase()
									.includes(searchTerm.trim().toLowerCase()))
					)}
			>
				<summary class="cursor-pointer text-sm font-medium"
					>Advanced provider limits</summary
				>
				<p class="mt-2 text-xs text-muted-foreground">
					Tune shared request capacity and retries only when provider limits require it.
				</p>
				<Field.FieldGroup class="mt-4"
					>{#each numericFields.slice(1) as field (field.key)}{@render numericSetting(
							field
						)}{/each}</Field.FieldGroup
				>
			</details>
			<p class="text-xs text-muted-foreground">
				AI providers and models are configured by your workspace administrator through the
				server configuration, not here.
			</p>
			<div
				class="flex items-center gap-2 text-xs text-muted-foreground"
				data-testid="settings-save-status"
				role={saveStatus === 'error' ? 'alert' : 'status'}
				aria-live="polite"
			>
				{#if saveStatus === 'saving'}<Spinner />{/if}
				<span>{saveStatusLabel}</span>
				{#if saveError && saveStatus === 'error'}<span>{saveError}</span>{/if}
			</div>
		{:else if activeTab === 'ai'}
			{#if aiProjectId}
				<AutonomySettings projectId={aiProjectId} />
			{:else}
				<p class="text-sm text-muted-foreground" data-testid="ai-settings-no-project">
					Open a project to change how far the AI may go in it.
				</p>
			{/if}
		{:else}
			<Field.Field orientation="horizontal">
				<Field.FieldContent
					><Field.FieldLabel>Theme</Field.FieldLabel><Field.FieldDescription
						>Follow your device or choose a fixed appearance.</Field.FieldDescription
					></Field.FieldContent
				>
				<Select.Root
					type="single"
					bind:value={() => userPrefersMode.current, (value) => setThemeMode(value)}
				>
					<Select.Trigger aria-label="Theme" class="w-32"
						>{themeModeLabel(userPrefersMode.current as ThemeMode)}</Select.Trigger
					>
					<Select.Content
						><Select.Group
							><Select.Item value="light" label="Light" /><Select.Item
								value="dark"
								label="Dark"
							/><Select.Item value="system" label="System" /></Select.Group
						></Select.Content
					>
				</Select.Root>
			</Field.Field>
		{/if}
	</form>
{/snippet}
