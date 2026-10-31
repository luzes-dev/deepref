<script lang="ts">
	import {
		Database,
		UserRound,
		Maximize2,
		Minimize2,
		Palette,
		Search,
		Sparkles,
		X
	} from '@lucide/svelte';
	import type { LucideIcon } from '@lucide/svelte';
	import type { SettingsDto } from '#lib/api/generated/models/index.js';
	import { createGetSettings } from '#lib/api/generated/settings/settings.js';
	import { StatePanel } from '@deepref/ui/layout';
	import { Button } from '@deepref/ui/button';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import * as Select from '@deepref/ui/select';
	import { setMode, userPrefersMode } from 'mode-watcher';
	import { page } from '$app/state';
	import AccountSettings from './AccountSettings.svelte';
	import AutonomySettings from '#lib/features/ai-autonomy/components/AutonomySettings.svelte';

	export type SettingsViewProps = {
		presentation?: 'page' | 'modal';
		onclose?: () => void;
		onexpand?: () => void;
		oncollapse?: () => void;
	};
	type DeploymentSettingKey =
		'default_max_depth' | 'max_concurrency' | 'rate_limit_per_second' | 'retry_attempts';
	type SettingsSection = 'ingestion' | 'ai' | 'appearance' | 'account';
	type ThemeMode = 'light' | 'dark' | 'system';

	let { presentation = 'page', onclose, onexpand, oncollapse }: SettingsViewProps = $props();

	// These limits are deployment-wide and only an operator may change them
	// (PATCH /settings is denied to every user), so they are shown read-only.
	const deploymentFields = [
		{
			key: 'default_max_depth',
			id: 'depth',
			label: 'Default max depth',
			description:
				'Default citation depth for new imports. Each level can multiply the number of records: depth 1 adds the articles they cite, and depth 2 can add thousands.'
		},
		{
			key: 'max_concurrency',
			id: 'concurrency',
			label: 'Max concurrency',
			description: 'Number of provider requests that may run concurrently.'
		},
		{
			key: 'rate_limit_per_second',
			id: 'rate',
			label: 'Rate limit per second',
			description: 'Provider request budget per second, shared globally.'
		},
		{
			key: 'retry_attempts',
			id: 'retry',
			label: 'Retry attempts',
			description: 'Attempts after a provider request fails.'
		}
	] as const satisfies readonly {
		key: DeploymentSettingKey;
		id: string;
		label: string;
		description: string;
	}[];
	const sectionItems: readonly {
		id: SettingsSection;
		label: string;
		description: string;
		searchText: string;
		icon: LucideIcon;
	}[] = [
		{
			id: 'account',
			label: 'Account',
			description: 'Workspace access, billing and privacy',
			searchText:
				'API keys machine access rotate revoke billing checkout portal privacy download export account',
			icon: UserRound
		},
		{
			id: 'ingestion',
			label: 'Ingestion',
			description: 'Provider and ingestion limits for this deployment',
			searchText: deploymentFields
				.flatMap((field) => [field.label, field.description])
				.join(' '),
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

	let activeTab = $state<SettingsSection>('ingestion');
	let searchTerm = $state('');

	const settings = $derived<SettingsDto | undefined>(settingsQueryResult.data?.data);
	const isLoading = $derived(settingsQueryResult.isPending && !settings);
	const loadError = $derived(
		!settingsQueryResult.data ? (settingsQueryResult.error?.message ?? '') : ''
	);
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

	function setThemeMode(value: string | undefined): void {
		if (value === 'light' || value === 'dark' || value === 'system') setMode(value);
	}

	function themeModeLabel(value: ThemeMode): string {
		return value === 'light' ? 'Light' : value === 'dark' ? 'Dark' : 'System';
	}
</script>

<svelte:head>
	<title>Settings · DeepRef</title>
	<meta
		name="description"
		content="Review deployment-wide ingestion limits and configure your account and appearance."
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
				{activeTab === 'account'
					? 'Account and workspace actions'
					: activeTab === 'ai'
						? 'This project · Changes save automatically.'
						: activeTab === 'ingestion'
							? 'Deployment limits · Read-only'
							: 'This device'}
			</p>
		</div>
		<div class="flex items-center gap-1">
			{#if onexpand}<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => onexpand?.()}
					aria-label="Open full settings"><Maximize2 /></Button
				>{/if}
			{#if oncollapse}<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => oncollapse?.()}
					aria-label="Collapse settings"><Minimize2 /></Button
				>{/if}
			{#if onclose}<Button
					variant="ghost"
					size="icon-sm"
					onclick={() => onclose?.()}
					aria-label="Close settings"><X /></Button
				>{/if}
		</div>
	</header>
	<div class="flex shrink-0 flex-wrap items-center justify-between gap-3 px-4 pb-3 sm:px-6">
		{@render settingsNavigation('flex flex-wrap items-center gap-1', '')}
		{@render searchSettings('relative block w-48')}
	</div>
	<section
		class="min-h-0 flex-1 overflow-auto px-4 py-4 sm:px-6"
		aria-labelledby="settings-title"
	>
		<div class="max-w-3xl">
			{#if activeTab === 'account'}<AccountSettings />
			{:else if isLoading}<div data-testid="settings-loading">
					<StatePanel
						state="loading"
						title="Loading application settings"
						description="Retrieving provider and ingestion limits."
					/>
				</div>
			{:else if loadError}<div data-testid="settings-load-error">
					{@render settingsLoadError()}
				</div>
			{:else if settings}{@render settingsBody(settings)}{/if}
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

{#snippet deploymentSetting(field: (typeof deploymentFields)[number], value: number)}
	<div
		class="grid grid-cols-[minmax(0,1fr)_minmax(4rem,8rem)] items-start gap-x-4 gap-y-1 max-sm:flex max-sm:flex-col"
	>
		<div class="flex flex-col gap-1">
			<dt id={`${field.id}-label`} class="text-sm font-medium">{field.label}</dt>
			<p id={`${field.id}-description`} class="text-xs text-muted-foreground">
				{field.description}
			</p>
		</div>
		<dd
			id={field.id}
			class="text-sm font-medium tabular-nums sm:text-right"
			aria-labelledby={`${field.id}-label`}
			aria-describedby={`${field.id}-description`}
			data-testid={`setting-${field.id}`}
		>
			{value}
		</dd>
	</div>
{/snippet}

{#snippet settingsBody(current: SettingsDto)}
	<div class="flex flex-col gap-6">
		{#if activeTab === 'ingestion'}
			<p class="text-xs text-muted-foreground" data-testid="settings-read-only-note">
				These limits apply to the whole deployment. Only the operator running DeepRef can
				change them, through the server configuration.
			</p>
			<dl class="flex flex-col gap-4" aria-label="Deployment ingestion limits">
				{#each deploymentFields as field (field.key)}
					{@render deploymentSetting(field, current[field.key])}
				{/each}
			</dl>
			<p class="text-xs text-muted-foreground">
				AI providers and models are configured by your workspace administrator through the
				server configuration, not here.
			</p>
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
	</div>
{/snippet}
