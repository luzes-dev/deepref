<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Field from '@deepref/ui/field';
	import * as InputGroup from '@deepref/ui/input-group';
	import * as NumberField from '@deepref/ui/number-field';
	import { Button } from '@deepref/ui/button';
	import { createCreateIngestion } from '#lib/api/generated/ingestions/ingestions.js';
	import { createGetSettings } from '#lib/api/generated/settings/settings.js';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import { openSettingsFromLink } from '#lib/features/settings/navigation.js';
	import { resolve } from '$app/paths';
	import PlayIcon from '@lucide/svelte/icons/play';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { IMPORT_FALLBACK_MAX_DEPTH, IMPORT_FORM_MAX_DEPTH } from '../constants';
	import { parseDoiList, pluralize, type LineIssue } from '../imports';
	import { parseRememberedDepth, resolveImportDepth } from '../ingestion-depth';

	const workspace = useProjectWorkspaceContext();
	const createIngestion = createCreateIngestion();
	const settingsQuery = createGetSettings();

	const MAX_LISTED_ISSUES = 5;
	const depthStorageKey = $derived(`deepref:ingestion-depth:${workspace.project.id}`);
	const settingsDefault = $derived(settingsQuery.data?.data.default_max_depth);
	// Remembered choice for this project, else the workspace Settings default, else the fallback.
	const depth = $derived(
		resolveImportDepth({ remembered: workspace.ingestionDepthChoice, settingsDefault })
	);
	// Typing and stepping stop at the form cap, but a Settings default above it is shown as set.
	const depthMax = $derived(Math.max(IMPORT_FORM_MAX_DEPTH, settingsDefault ?? 0));
	// Without a remembered choice the form would start from the fallback, so it waits for Settings.
	const waitingForSettings = $derived(settingsQuery.isPending && depth.source !== 'remembered');

	// Restore the depth last chosen for this project so it survives reloads.
	$effect(() => {
		try {
			const remembered = parseRememberedDepth(localStorage.getItem(depthStorageKey));
			if (remembered !== undefined) workspace.ingestionDepthChoice = remembered;
		} catch {
			// Storage can be unavailable; the Settings default applies.
		}
	});

	function rememberDepth(value: number) {
		if (!Number.isSafeInteger(value) || value < 0) return;
		workspace.ingestionDepthChoice = value;
		try {
			localStorage.setItem(depthStorageKey, String(value));
		} catch {
			// Remembering the depth is a convenience only.
		}
	}

	const parsed = $derived(parseDoiList(workspace.ingestionDraft.dois));
	let submitIssues = $state<LineIssue[]>([]);

	function describeIssue(issue: LineIssue): string {
		return `Line ${issue.line}: “${issue.value}” ${issue.message}`;
	}

	const depthHint = $derived(
		depth.depth === 0
			? 'Imports only the articles you paste.'
			: depth.depth === 1
				? 'Also imports the articles they cite, often dozens per article.'
				: 'Follows citations of citations and can reach thousands of records to screen.'
	);

	const depthSourceText = $derived(
		settingsQuery.isPending
			? 'Loading the default from Settings…'
			: settingsDefault === undefined
				? `Default from Settings could not be read, so this starts at ${IMPORT_FALLBACK_MAX_DEPTH}.`
				: depth.source === 'remembered'
					? `Default from Settings: ${settingsDefault} · Remembered for this project`
					: `Default from Settings: ${settingsDefault}`
	);

	async function submit() {
		if (parsed.issues.length > 0) {
			submitIssues = parsed.issues;
			return;
		}
		submitIssues = [];
		if (parsed.dois.length === 0 || waitingForSettings) return;
		try {
			const result = await createIngestion.mutateAsync({
				data: {
					project_id: workspace.project.id,
					seed_dois: parsed.dois,
					max_depth: depth.depth,
					metadata_provider: 'crossref',
					citation_provider: 'crossref'
				}
			});
			workspace.ingestionDraft.dois = '';
			workspace.openIngestion(result.data.id);
		} catch (error) {
			notifyError('Ingestion could not be started', error);
		}
	}
</script>

<form
	class="flex max-w-3xl flex-col gap-4"
	data-testid="doi-import-form"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<Field.Field>
		<Field.FieldLabel for="dois" class="sr-only">Article DOIs</Field.FieldLabel>
		<InputGroup.Root>
			<InputGroup.Textarea
				id="dois"
				rows={6}
				placeholder="One DOI per line, for example 10.xxxx/xxxxxx"
				aria-invalid={submitIssues.length > 0 ? 'true' : undefined}
				aria-describedby="dois-status"
				bind:value={workspace.ingestionDraft.dois}
			/>
		</InputGroup.Root>
		<Field.FieldDescription>
			<span id="dois-status" aria-live="polite">
				{#if parsed.dois.length === 0 && parsed.issues.length === 0}
					One DOI per line, or separated by commas. A doi.org link works too.
				{:else}
					{pluralize(parsed.dois.length, 'unique DOI')} recognised{parsed.duplicates > 0
						? ` · ${parsed.duplicates} repeated ignored`
						: ''}{parsed.issues.length > 0
						? ` · ${pluralize(parsed.issues.length, 'line')} not a DOI`
						: ''}
				{/if}
			</span>
		</Field.FieldDescription>
		{#if parsed.issues.length > 0}
			<ul class="flex flex-col gap-1 text-xs text-destructive" data-testid="doi-issues">
				{#each parsed.issues.slice(0, MAX_LISTED_ISSUES) as issue (issue.line + issue.value)}
					<li>{describeIssue(issue)}</li>
				{/each}
				{#if parsed.issues.length > MAX_LISTED_ISSUES}
					<li>and {parsed.issues.length - MAX_LISTED_ISSUES} more</li>
				{/if}
			</ul>
		{/if}
	</Field.Field>

	{#if submitIssues.length > 0}
		<Alert.Root variant="destructive" role="alert" data-testid="doi-submit-error">
			<CircleAlertIcon />
			<Alert.Title>Nothing was imported</Alert.Title>
			<Alert.Description>
				Fix the lines that are not DOIs, then import again:
				{submitIssues.map(describeIssue).join('; ')}.
			</Alert.Description>
		</Alert.Root>
	{/if}

	<div class="flex flex-wrap items-start justify-between gap-4">
		<div class="max-w-sm">
			<Field.Field>
				<Field.FieldLabel for="max-depth">Maximum citation depth</Field.FieldLabel>
				<NumberField.Root
					bind:value={() => depth.depth, (value) => rememberDepth(value)}
					min={0}
					max={depthMax}
				>
					<NumberField.Group
						><NumberField.Decrement /><NumberField.Input
							id="max-depth"
						/><NumberField.Increment /></NumberField.Group
					>
				</NumberField.Root>
				<Field.FieldDescription
					>0 = only these articles · 1 = also the articles they cite · 2+ = citations of
					citations. {depthHint}</Field.FieldDescription
				>
				<p
					class="flex flex-wrap items-baseline gap-x-2 text-xs text-muted-foreground"
					data-testid="depth-default-source"
				>
					<span>{depthSourceText}</span>
					<a
						href={resolve('/settings')}
						onclick={openSettingsFromLink}
						class="text-primary underline underline-offset-4 focus-visible:outline-2 focus-visible:outline-ring"
						data-testid="depth-settings-link">Change in Settings</a
					>
				</p>
				{#if depth.depth >= 2}
					<p
						class="flex items-start gap-1.5 text-xs text-warning"
						role="status"
						data-testid="depth-warning"
					>
						<TriangleAlertIcon class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
						Depth {depth.depth} can add thousands of records and take a long time. Start with
						1 unless you need the wider network.
					</p>
				{/if}
			</Field.Field>
		</div>
		<Button
			type="submit"
			disabled={parsed.dois.length === 0 || createIngestion.isPending || waitingForSettings}
		>
			<PlayIcon data-icon="inline-start" />{createIngestion.isPending
				? 'Starting ingestion…'
				: parsed.dois.length > 1
					? `Import ${parsed.dois.length} articles`
					: 'Import articles'}
		</Button>
	</div>
</form>
