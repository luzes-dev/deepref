<script lang="ts">
	import * as Tabs from '@deepref/ui/tabs';
	import * as Alert from '@deepref/ui/alert';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import ProtocolDocument from './ProtocolDocument.svelte';
	import * as Empty from '@deepref/ui/empty';
	import * as Field from '@deepref/ui/field';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Spinner } from '@deepref/ui/spinner';
	import { Textarea } from '@deepref/ui/textarea';
	import * as Select from '@deepref/ui/select';
	import { notifyError } from '#lib/features/notifications/toast.js';
	import { useProjectWorkspaceContext } from '#lib/features/projects/context.svelte.js';
	import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
	import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SaveIcon from '@lucide/svelte/icons/save';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import CheckCircle2Icon from '@lucide/svelte/icons/circle-check';
	import ListChecksIcon from '@lucide/svelte/icons/list-checks';
	import { beforeNavigate } from '$app/navigation';
	import { page } from '$app/state';
	import { tick } from 'svelte';
	import { createForm } from '@tanstack/svelte-form';
	import { useQueryClient } from '@tanstack/svelte-query';
	import {
		createGetProjectReviewProtocol,
		createPublishProjectReviewProtocol,
		createSaveProjectReviewProtocol,
		getGetProjectReviewProtocolQueryKey,
		getListProjectReviewProtocolVersionsQueryKey,
		isConflict,
		type PublishProtocolRequest
	} from '../api';
	import {
		CRITERION_DIMENSIONS,
		CRITERION_KINDS,
		CRITERION_STAGES,
		FRAMEWORK_FIELDS,
		FRAMEWORK_KINDS,
		criterionDimensionLabel,
		criterionKindLabel,
		criterionStageLabel,
		duplicateCustomKeys,
		frameworkFieldsForKind,
		frameworkLabel,
		humanizeKey,
		isCriterionDimension,
		isCriterionKind,
		isCriterionStage,
		isFrameworkKind,
		isRequiredFrameworkField
	} from '../codecs';
	import {
		buildSaveProtocolRequest,
		cloneCustomFields,
		customFieldsFromRecord,
		emptyProtocolDraft,
		newProtocolDraft,
		protocolDraftFromDto,
		protocolDraftStorageKey,
		protocolIssues,
		restoreStoredDraft,
		serializeStoredDraft,
		validateProtocolDraft,
		type DraftClientIdFactory,
		type ProtocolDraft,
		type ProtocolIssue,
		type ProtocolTab
	} from '../draft';

	const projectId = $derived(page.params.projectId ?? '');
	const queryClient = useQueryClient();
	const workspace = useProjectWorkspaceContext();
	const projectDetails = $derived(
		workspace.project
			? { name: workspace.project.name, description: workspace.project.description }
			: undefined
	);
	const protocolQuery = createGetProjectReviewProtocol(
		() => projectId,
		() => ({
			query: { retry: false }
		})
	);
	const saveProtocol = createSaveProjectReviewProtocol();
	const publishProtocol = createPublishProjectReviewProtocol();

	let clientCriterionId = 0;
	let clientFieldId = 0;
	const nextClientId: DraftClientIdFactory = (kind) =>
		kind === 'criterion' ? `criterion-${clientCriterionId++}` : `field-${clientFieldId++}`;

	let hydratedKey = $state<string | undefined>(undefined);
	let amending = $state(false);
	let reconciling = $state(false);
	let forceHydrate = $state(false);
	let tab = $state<ProtocolTab>('question');
	let attempted = $state(false);
	let restored = $state(false);
	let baseDraft: ProtocolDraft | undefined;
	let autosaveTimer: ReturnType<typeof setTimeout> | undefined;
	const AUTOSAVE_DELAY_MS = 1500;

	// `null` is the answer for a project that has no protocol yet: the editor starts a new draft.
	const protocol = $derived(protocolQuery.data?.data);

	async function validateUniqueProtocolName(
		value: string,
		signal: AbortSignal
	): Promise<string | undefined> {
		await new Promise<void>((resolve) => {
			let settled = false;
			const finish = () => {
				if (settled) return;
				settled = true;
				signal.removeEventListener('abort', finish);
				resolve();
			};
			const timeout = setTimeout(finish, 120);
			signal.addEventListener(
				'abort',
				() => {
					clearTimeout(timeout);
					finish();
				},
				{ once: true }
			);
		});
		if (signal.aborted) return undefined;
		return value.trim().toLowerCase() === 'duplicate'
			? 'Choose a protocol name that is unique within this workspace.'
			: undefined;
	}

	const form = createForm(() => ({
		formId: 'deepref-review-protocol',
		defaultValues: emptyProtocolDraft(),
		canSubmitWhenInvalid: true,
		onSubmit: async ({ value }) => saveValues(value)
	}));

	const status = form.useSelector((state) => state.values.status);
	const protocolId = form.useSelector((state) => state.values.id);
	const frameworkKind = form.useSelector((state) => state.values.frameworkKind);
	const frameworkFields = $derived(FRAMEWORK_FIELDS[frameworkKind.current]);
	const isDirty = form.useSelector((state) => state.isDirty);
	const formReady = form.useSelector(
		(state) =>
			!state.isValidating && state.isValid && validateProtocolDraft(state.values).length === 0
	);
	const isSubmitting = form.useSelector((state) => state.isSubmitting);
	// Selects a serialized snapshot, so the effects below re-run only when the draft's content
	// changes. svelte-store's default comparator is patched to Object.is (see
	// patches/@tanstack__svelte-store@0.12.1.patch): same re-render behaviour, no dev-only
	// state_proxy_equality_mismatch warning. Upstream's $state.raw change would stop array fields
	// from re-rendering after a form reset, so do not swap it in.
	const valuesJson = form.useSelector((state) => JSON.stringify(state.values));
	const values = $derived(JSON.parse(valuesJson.current) as ProtocolDraft);

	const isPublished = $derived(status.current === 'published' && !amending);
	const editable = $derived(!isPublished && status.current !== 'superseded');
	const errorMessage = $derived(protocolQuery.error?.message);
	const isPending = $derived(
		saveProtocol.isPending || publishProtocol.isPending || isSubmitting.current
	);
	const canPublish = $derived(editable && !isPending);
	const duplicateKeys = $derived(duplicateCustomKeys(values.customFrameworkFields));
	const issues = $derived(protocolIssues(values));
	const statusLabel = $derived(
		isPending
			? 'Saving…'
			: amending
				? 'Kept on this device until you save'
				: isDirty.current
					? 'Unsaved changes'
					: protocolId.current
						? 'Saved'
						: 'Not saved yet'
	);

	$effect(() => {
		const current = protocol;
		// The new-draft key includes whether the project has loaded, so its name and question
		// are pre-filled once the project details arrive.
		const nextKey = current
			? `${current.id}:${current.revision}`
			: current === null
				? `new:${projectId}:${projectDetails ? 'project' : 'loading'}`
				: undefined;

		if (
			reconciling ||
			nextKey === undefined ||
			nextKey === hydratedKey ||
			(isDirty.current && !forceHydrate)
		)
			return;
		if (current && current.revision < form.state.values.revision) return;
		const base = current
			? protocolDraftFromDto(current, nextClientId)
			: newProtocolDraft(projectDetails);
		form.reset(base);
		baseDraft = base;
		hydratedKey = nextKey;
		forceHydrate = false;
		amending = false;
		restored = false;
		restoreLocalDraft(base);
	});

	function readLocal(key: string): string | null {
		try {
			return localStorage.getItem(key);
		} catch {
			return null;
		}
	}

	function writeLocal(key: string, value: string | undefined): void {
		try {
			if (value === undefined) localStorage.removeItem(key);
			else localStorage.setItem(key, value);
		} catch {
			// Storage can be unavailable; the server autosave still applies.
		}
	}

	function restoreLocalDraft(base: ProtocolDraft): void {
		const raw = readLocal(protocolDraftStorageKey(projectId, base));
		const stored = raw ? restoreStoredDraft(raw, base, nextClientId) : undefined;
		if (!stored) return;
		const draft = stored.draft;
		form.setFieldValue('name', draft.name);
		form.setFieldValue('objective', draft.objective);
		form.setFieldValue('question', draft.question);
		form.setFieldValue('frameworkFieldSnapshots', {});
		form.setFieldValue('customFrameworkSnapshot', undefined);
		form.setFieldValue('customFrameworkFields', draft.customFrameworkFields);
		form.setFieldValue('frameworkFields', draft.frameworkFields);
		form.setFieldValue('frameworkKind', draft.frameworkKind);
		form.setFieldValue('criteria', draft.criteria);
		amending = stored.amending;
		restored = true;
	}

	function discardRestored(): void {
		if (!baseDraft) return;
		writeLocal(protocolDraftStorageKey(projectId, baseDraft), undefined);
		form.reset(baseDraft);
		amending = false;
		restored = false;
		attempted = false;
	}

	// Keep the in-progress form on this device and schedule a quiet server autosave.
	$effect(() => {
		const snapshot = values;
		const dirty = isDirty.current;
		if (hydratedKey === undefined) return;
		writeLocal(
			protocolDraftStorageKey(projectId, snapshot),
			dirty && editable ? serializeStoredDraft(snapshot, amending) : undefined
		);
		scheduleAutosave();
		return () => clearTimeout(autosaveTimer);
	});

	function scheduleAutosave(): void {
		clearTimeout(autosaveTimer);
		if (!isDirty.current || !editable) return;
		autosaveTimer = setTimeout(() => void autosave(), AUTOSAVE_DELAY_MS);
	}

	async function autosave(): Promise<void> {
		clearTimeout(autosaveTimer);
		// An amendment becomes a new server version only on an explicit save; until then it is
		// kept locally (and restored on return) so it can still be discarded.
		if (amending) return;
		if (!editable || !isDirty.current || isPending || !formReady.current) return;
		await saveValues(form.state.values);
	}

	function flushAutosave(): void {
		void autosave();
	}

	beforeNavigate((navigation) => {
		if (navigation.shallow && navigation.type === 'goto') return;
		if (!isDirty.current || !editable) return;
		flushAutosave();
		if (navigation.type === 'leave') {
			navigation.cancel();
		} else if (
			!formReady.current &&
			!window.confirm(
				'This protocol is not complete yet. Your changes stay in this browser and are restored when you come back. Leave anyway?'
			)
		) {
			navigation.cancel();
		}
	});

	/** For free-text fields the researcher is typing in, errors may appear as they type. */
	function showTypedErrors(meta: { isTouched: boolean }): boolean {
		return attempted || meta.isTouched;
	}

	function showErrors(meta: { isBlurred: boolean }): boolean {
		return attempted || meta.isBlurred;
	}

	async function goToIssue(issue: ProtocolIssue): Promise<void> {
		tab = issue.tab;
		await tick();
		if (issue.targetId) document.getElementById(issue.targetId)?.focus();
	}

	/** Reveals what is missing when the researcher tries to save or publish an incomplete protocol. */
	async function revealIssues(): Promise<void> {
		attempted = true;
		await form.validateAllFields('change');
		const first = issues[0];
		if (first) await goToIssue(first);
	}

	function changeFramework(value: string | undefined): void {
		if (!value || !isFrameworkKind(value)) return;
		const current = form.state.values;
		const previousKind = current.frameworkKind;
		const snapshots = { ...current.frameworkFieldSnapshots };
		let customSnapshot = current.customFrameworkSnapshot
			? cloneCustomFields(current.customFrameworkSnapshot)
			: undefined;
		if (previousKind === 'custom') {
			customSnapshot = cloneCustomFields(current.customFrameworkFields);
		} else {
			snapshots[previousKind] = frameworkFieldsForKind(previousKind, {
				...current.frameworkFields
			});
		}

		let nextCustomFields: typeof current.customFrameworkFields;
		let nextFrameworkFields: typeof current.frameworkFields;
		if (value === 'custom') {
			nextCustomFields = cloneCustomFields(
				customSnapshot ?? customFieldsFromRecord(current.frameworkFields, nextClientId)
			);
			nextFrameworkFields = {};
		} else {
			nextFrameworkFields = frameworkFieldsForKind(
				value,
				snapshots[value] ?? { ...current.frameworkFields }
			);
			nextCustomFields = [];
		}
		form.setFieldValue('frameworkFieldSnapshots', snapshots);
		form.setFieldValue('customFrameworkSnapshot', customSnapshot);
		form.setFieldValue('customFrameworkFields', nextCustomFields);
		form.setFieldValue('frameworkFields', nextFrameworkFields);
		form.setFieldValue('frameworkKind', value);
	}

	async function saveValues(value: typeof form.state.values): Promise<void> {
		const request = buildSaveProtocolRequest(value);
		const sent = JSON.stringify(request);
		try {
			const result = await saveProtocol.mutateAsync({ projectId, data: request });
			writeLocal(protocolDraftStorageKey(projectId, value), undefined);
			if (JSON.stringify(buildSaveProtocolRequest(form.state.values)) === sent) {
				form.reset(protocolDraftFromDto(result.data, nextClientId));
				amending = false;
				restored = false;
			} else {
				// The researcher kept typing while saving: only advance the revision.
				form.setFieldValue('id', result.data.id);
				form.setFieldValue('revision', result.data.revision);
				form.setFieldValue('version', result.data.version);
				form.setFieldValue('status', 'draft');
				scheduleAutosave();
			}
			hydratedKey = `${result.data.id}:${result.data.revision}`;
			queryClient.setQueryData(getGetProjectReviewProtocolQueryKey(projectId), {
				data: result.data
			});
			await queryClient.invalidateQueries({
				queryKey: getGetProjectReviewProtocolQueryKey(projectId),
				refetchType: 'none'
			});
		} catch (error) {
			notifyError(
				isConflict(error) ? 'Protocol changed elsewhere' : 'Protocol could not be saved',
				error,
				'The protocol draft could not be saved.',
				{
					action: isConflict(error)
						? { label: 'Refresh', onClick: () => void reconcileFromServer() }
						: undefined
				}
			);
		}
	}

	async function save(): Promise<void> {
		if (!editable || isPending) return;
		if (!formReady.current) {
			await revealIssues();
			return;
		}
		clearTimeout(autosaveTimer);
		await form.handleSubmit();
	}

	async function publish(): Promise<void> {
		if (!canPublish) return;
		// Publishing is final, so every checklist item must be resolved, not only the required fields.
		if (!formReady.current || issues.length > 0) {
			await revealIssues();
			return;
		}
		clearTimeout(autosaveTimer);
		if (isDirty.current || !form.state.values.id || status.current !== 'draft') {
			await saveValues(form.state.values);
			if (isDirty.current || status.current !== 'draft') return;
		}
		const protocolVersionId = form.state.values.id;
		if (!protocolVersionId) return;
		const request: PublishProtocolRequest = {
			protocol_version_id: protocolVersionId,
			expected_revision: form.state.values.revision
		};
		try {
			const result = await publishProtocol.mutateAsync({ projectId, data: request });
			writeLocal(protocolDraftStorageKey(projectId, form.state.values), undefined);
			form.reset(protocolDraftFromDto(result.data, nextClientId));
			restored = false;
			attempted = false;
			hydratedKey = `${result.data.id}:${result.data.revision}`;
			queryClient.setQueryData(getGetProjectReviewProtocolQueryKey(projectId), {
				data: result.data
			});
			await queryClient.invalidateQueries({
				queryKey: getGetProjectReviewProtocolQueryKey(projectId),
				refetchType: 'none'
			});
			await queryClient.invalidateQueries({
				queryKey: getListProjectReviewProtocolVersionsQueryKey(projectId)
			});
			await tick();
			document.querySelector('[data-testid="protocol-document"]')?.scrollIntoView();
			window.scrollTo({ top: 0 });
		} catch (error) {
			notifyError(
				'Protocol could not be published',
				error,
				'The protocol version could not be published.',
				{
					action: isConflict(error)
						? { label: 'Refresh', onClick: () => void reconcileFromServer() }
						: undefined
				}
			);
		}
	}

	async function reconcileFromServer(): Promise<void> {
		saveProtocol.reset();
		publishProtocol.reset();
		writeLocal(protocolDraftStorageKey(projectId, form.state.values), undefined);
		restored = false;
		attempted = false;
		amending = false;
		hydratedKey = undefined;
		forceHydrate = true;
		reconciling = true;
		try {
			await protocolQuery.refetch();
		} finally {
			reconciling = false;
		}
	}

	function beginAmendment(): void {
		if (status.current !== 'published') return;
		amending = true;
	}
</script>

<svelte:head>
	<title>Protocol · DeepRef</title>
	<meta
		name="description"
		content="Versioned protocol and eligibility criteria for a DeepRef evidence workspace."
	/>
</svelte:head>

<PageTemplate testId="protocol-page" maxWidth="wide">
	{#if errorMessage}
		<Alert.Root variant="destructive" role="alert">
			<Alert.Title>Protocol unavailable</Alert.Title>
			<Alert.Description>{errorMessage}</Alert.Description>
		</Alert.Root>
	{:else if protocolQuery.isPending}
		<section class="workflow-section border-primary/15">
			<div class="flex min-w-0 items-center gap-3 py-10" aria-live="polite">
				<Spinner /> Loading protocol…
			</div>
		</section>
	{:else if protocol === undefined}
		<section class="workflow-section border-destructive/30">
			<div class="flex min-w-0 flex-col gap-3 py-10">
				<div class="flex items-center gap-2">
					<BookOpenIcon class="text-destructive" aria-hidden="true" />
					<p class="font-medium">Protocol could not be loaded.</p>
				</div>
				<Button
					type="button"
					variant="outline"
					onclick={() => void protocolQuery.refetch()}
				>
					<RefreshCwIcon data-icon="inline-start" />Retry
				</Button>
			</div>
		</section>
	{:else}
		{#if isPublished && protocol}
			<ProtocolDocument {protocol} {projectId} onAmend={beginAmendment} />
		{:else}
			<form
				class="flex min-w-0 flex-col gap-6"
				onsubmit={(event) => {
					event.preventDefault();
					event.stopPropagation();
					void save();
				}}
			>
				<Tabs.Root
					value={tab}
					onValueChange={(value) => {
						tab = value as ProtocolTab;
						flushAutosave();
					}}
					class="min-w-0 gap-6"
				>
					<Tabs.List
						variant="line"
						aria-label="Page sections"
						class="max-w-full justify-start overflow-x-auto border-b"
					>
						<Tabs.Trigger value="question">Research question</Tabs.Trigger>
						<Tabs.Trigger value="framework">Framework</Tabs.Trigger>
						<Tabs.Trigger value="criteria">Eligibility criteria</Tabs.Trigger>
					</Tabs.List>
					<Tabs.Content value="question">
						<section class="workflow-section border-primary/15">
							<div class="min-w-0">
								<Field.Group>
									<form.Field
										name="name"
										validators={{
											onChange: ({ value }) =>
												!value.trim()
													? 'Give the protocol a name.'
													: undefined,
											onChangeAsync: ({ value, signal }) =>
												validateUniqueProtocolName(value, signal),
											onChangeAsyncDebounceMs: 350
										}}
									>
										{#snippet children(field)}
											<Field.Field
												data-invalid={showTypedErrors(field.state.meta) &&
													field.state.meta.errors.length > 0}
											>
												<Field.Label for="protocol-name">Name</Field.Label>
												<Input
													id="protocol-name"
													name={field.name}
													value={field.state.value}
													onblur={field.handleBlur}
													oninput={(event) =>
														field.handleChange(
															event.currentTarget.value
														)}
													disabled={!editable}
													aria-invalid={field.state.meta.errors.length >
														0}
													aria-busy={field.state.meta.isValidating}
												/>
												{#if showTypedErrors(field.state.meta) && field.state.meta.errors[0]}<Field.FieldError
														>{field.state.meta
															.errors[0]}</Field.FieldError
													>{/if}
											</Field.Field>
										{/snippet}
									</form.Field>
									<form.Field
										name="objective"
										validators={{
											onChange: ({ value }) =>
												!value.trim()
													? 'Add the review objective.'
													: undefined
										}}
									>
										{#snippet children(field)}
											<Field.Field
												data-invalid={showErrors(field.state.meta) &&
													field.state.meta.errors.length > 0}
											>
												<Field.Label for="protocol-objective"
													>Objective</Field.Label
												>
												<Textarea
													id="protocol-objective"
													name={field.name}
													value={field.state.value}
													onblur={field.handleBlur}
													oninput={(event) =>
														field.handleChange(
															event.currentTarget.value
														)}
													class="min-h-28"
													disabled={!editable}
													aria-invalid={field.state.meta.errors.length >
														0}
												/>
												{#if showErrors(field.state.meta) && field.state.meta.errors[0]}<Field.FieldError
														>{field.state.meta
															.errors[0]}</Field.FieldError
													>{/if}
											</Field.Field>
										{/snippet}
									</form.Field>
									<form.Field
										name="question"
										validators={{
											onChange: ({ value }) =>
												!value.trim()
													? 'Add the research question.'
													: undefined
										}}
									>
										{#snippet children(field)}
											<Field.Field
												data-invalid={showErrors(field.state.meta) &&
													field.state.meta.errors.length > 0}
											>
												<Field.Label for="protocol-question"
													>Question</Field.Label
												>
												<Textarea
													id="protocol-question"
													name={field.name}
													value={field.state.value}
													onblur={field.handleBlur}
													oninput={(event) =>
														field.handleChange(
															event.currentTarget.value
														)}
													class="min-h-28"
													disabled={!editable}
													aria-invalid={field.state.meta.errors.length >
														0}
												/>
												{#if showErrors(field.state.meta) && field.state.meta.errors[0]}<Field.FieldError
														>{field.state.meta
															.errors[0]}</Field.FieldError
													>{/if}
											</Field.Field>
										{/snippet}
									</form.Field>
								</Field.Group>
							</div>
						</section>
					</Tabs.Content>
					<Tabs.Content value="framework">
						<section class="workflow-section border-primary/15">
							<div class="flex min-w-0 flex-col gap-4">
								<form.Field name="frameworkKind">
									{#snippet children(field)}
										<Field.Field>
											<Field.Label>Framework</Field.Label>
											<Select.Root
												type="single"
												value={field.state.value}
												onValueChange={(value) => {
													if (value && isFrameworkKind(value)) {
														changeFramework(value);
														field.handleChange(value);
													}
												}}
											>
												<Select.Trigger disabled={!editable}
													>{frameworkLabel(
														field.state.value
													)}</Select.Trigger
												>
												<Select.Content>
													<Select.Group>
														{#each FRAMEWORK_KINDS as kind (kind)}
															<Select.Item
																value={kind}
																label={frameworkLabel(kind)}
															/>
														{/each}
													</Select.Group>
												</Select.Content>
											</Select.Root>
										</Field.Field>
									{/snippet}
								</form.Field>
								{#if frameworkKind.current === 'custom'}
									<form.Field name="customFrameworkFields" mode="array">
										{#snippet children(arrayField)}
											<div class="flex flex-col gap-3">
												{#each arrayField.state.value as field, index (field.clientId)}
													<div
														class="grid gap-2 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1fr)_auto]"
													>
														<form.Field
															name={`customFrameworkFields[${index}].key`}
															validators={{
																onChange: ({ value }) =>
																	!value.trim()
																		? 'Field name is required.'
																		: undefined
															}}
														>
															{#snippet children(keyField)}
																<Input
																	aria-label={`Custom framework field ${field.clientId} name`}
																	value={keyField.state.value}
																	placeholder="Field name"
																	disabled={!editable}
																	onblur={keyField.handleBlur}
																	oninput={(event) =>
																		keyField.handleChange(
																			event.currentTarget
																				.value
																		)}
																	aria-invalid={keyField.state
																		.meta.errors.length > 0}
																/>
															{/snippet}
														</form.Field>
														<form.Field
															name={`customFrameworkFields[${index}].value`}
															validators={{
																onChange: ({ value }) =>
																	!value.trim()
																		? 'Definition is required.'
																		: undefined
															}}
														>
															{#snippet children(valueField)}
																<Input
																	aria-label={`Custom framework field ${field.clientId} definition`}
																	value={valueField.state.value}
																	placeholder="Definition"
																	disabled={!editable}
																	onblur={valueField.handleBlur}
																	oninput={(event) =>
																		valueField.handleChange(
																			event.currentTarget
																				.value
																		)}
																	aria-invalid={valueField.state
																		.meta.errors.length > 0}
																/>
															{/snippet}
														</form.Field>
														<Button
															type="button"
															variant="ghost"
															size="icon"
															aria-label="Remove framework field"
															disabled={!editable}
															onclick={() =>
																arrayField.removeValue(index)}
															><Trash2Icon /></Button
														>
													</div>
												{/each}
												{#if duplicateKeys.length > 0}
													<Field.FieldError
														>Custom framework field names must be
														unique: {duplicateKeys.join(
															', '
														)}.</Field.FieldError
													>
												{/if}
												<Button
													type="button"
													variant="outline"
													class="w-fit"
													disabled={!editable}
													onclick={() =>
														arrayField.pushValue({
															clientId: nextClientId('field'),
															key: '',
															value: ''
														})}
												>
													<PlusIcon data-icon="inline-start" />Add field
												</Button>
											</div>
										{/snippet}
									</form.Field>
								{:else}
									<Field.Group class="sm:grid sm:grid-cols-2">
										{#each frameworkFields as field (field)}
											<form.Field
												name={`frameworkFields.${field}`}
												validators={{
													onChange: ({ value }) =>
														isRequiredFrameworkField(
															frameworkKind.current,
															field
														) && !value.trim()
															? `${humanizeKey(field)} is required.`
															: undefined
												}}
											>
												{#snippet children(ff)}
													<Field.Field
														data-invalid={ff.state.meta.errors.length >
															0}
													>
														<Field.Label
															for={`framework-field-${field}`}
															>{humanizeKey(field)}</Field.Label
														>
														<Input
															id={`framework-field-${field}`}
															value={ff.state.value}
															disabled={!editable}
															onblur={ff.handleBlur}
															oninput={(event) =>
																ff.handleChange(
																	event.currentTarget.value
																)}
															aria-invalid={ff.state.meta.errors
																.length > 0}
														/>
														{#if showErrors(ff.state.meta) && ff.state.meta.errors[0]}<Field.FieldError
																>{ff.state.meta
																	.errors[0]}</Field.FieldError
															>{/if}
													</Field.Field>
												{/snippet}
											</form.Field>
										{/each}
									</Field.Group>
								{/if}
							</div>
						</section>
					</Tabs.Content>
					<Tabs.Content value="criteria">
						<section class="workflow-section border-primary/15">
							<form.Field name="criteria" mode="array">
								{#snippet children(criteriaField)}
									<div class="flex flex-col gap-3">
										{#each criteriaField.state.value as criterion, index (criterion.clientId)}
											<div
												class="flex flex-col gap-2 rounded-lg border border-border/70 p-3 transition-colors hover:border-border"
											>
												<div class="flex flex-wrap items-center gap-2">
													<form.Field name={`criteria[${index}].kind`}>
														{#snippet children(kindField)}
															<Select.Root
																type="single"
																value={kindField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionKind(value)
																	)
																		kindField.handleChange(
																			value
																		);
																}}
															>
																<Select.Trigger
																	size="sm"
																	aria-label="Type"
																	id={`criterion-kind-${criterion.clientId}`}
																	disabled={!editable}
																	>{criterionKindLabel(
																		kindField.state.value
																	)}</Select.Trigger
																>
																<Select.Content
																	><Select.Group>
																		{#each CRITERION_KINDS as item (item)}<Select.Item
																				value={item}
																				label={criterionKindLabel(
																					item
																				)}
																			/>{/each}
																	</Select.Group></Select.Content
																>
															</Select.Root>
														{/snippet}
													</form.Field>
													<form.Field name={`criteria[${index}].stage`}>
														{#snippet children(stageField)}
															<Select.Root
																type="single"
																value={stageField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionStage(value)
																	)
																		stageField.handleChange(
																			value
																		);
																}}
															>
																<Select.Trigger
																	size="sm"
																	aria-label="Screening stage"
																	id={`criterion-stage-${criterion.clientId}`}
																	disabled={!editable}
																	>{criterionStageLabel(
																		stageField.state.value
																	)}</Select.Trigger
																>
																<Select.Content
																	><Select.Group>
																		{#each CRITERION_STAGES as item (item)}<Select.Item
																				value={item}
																				label={criterionStageLabel(
																					item
																				)}
																			/>{/each}
																	</Select.Group></Select.Content
																>
															</Select.Root>
														{/snippet}
													</form.Field>
													<form.Field
														name={`criteria[${index}].dimension`}
													>
														{#snippet children(dimField)}
															<Select.Root
																type="single"
																value={dimField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionDimension(value)
																	)
																		dimField.handleChange(
																			value
																		);
																}}
															>
																<Select.Trigger
																	size="sm"
																	aria-label="Dimension"
																	id={`criterion-dim-${criterion.clientId}`}
																	disabled={!editable}
																	>{criterionDimensionLabel(
																		dimField.state.value
																	)}</Select.Trigger
																>
																<Select.Content
																	><Select.Group>
																		{#each CRITERION_DIMENSIONS as item (item)}<Select.Item
																				value={item}
																				label={criterionDimensionLabel(
																					item
																				)}
																			/>{/each}
																	</Select.Group></Select.Content
																>
															</Select.Root>
														{/snippet}
													</form.Field>
													<div class="ml-auto flex items-center gap-0.5">
														<Button
															type="button"
															variant="ghost"
															size="icon-xs"
															aria-label="Move criterion up"
															disabled={!editable || index === 0}
															onclick={() =>
																criteriaField.moveValue(
																	index,
																	index - 1
																)}
														>
															<ArrowUpIcon />
														</Button>
														<Button
															type="button"
															variant="ghost"
															size="icon-xs"
															aria-label="Move criterion down"
															disabled={!editable ||
																index ===
																	criteriaField.state.value
																		.length -
																		1}
															onclick={() =>
																criteriaField.moveValue(
																	index,
																	index + 1
																)}
														>
															<ArrowDownIcon />
														</Button>
														<Button
															type="button"
															variant="ghost"
															size="icon-xs"
															aria-label="Remove criterion"
															disabled={!editable}
															onclick={() =>
																criteriaField.removeValue(index)}
														>
															<Trash2Icon />
														</Button>
													</div>
												</div>
												<form.Field
													name={`criteria[${index}].label`}
													validators={{
														onChange: ({ value }) =>
															!value.trim()
																? 'Label is required.'
																: undefined
													}}
												>
													{#snippet children(labelField)}
														<Input
															id={`criterion-label-${criterion.clientId}`}
															aria-label="Label"
															placeholder="Label"
															value={labelField.state.value}
															disabled={!editable}
															onblur={labelField.handleBlur}
															oninput={(event) =>
																labelField.handleChange(
																	event.currentTarget.value
																)}
															aria-invalid={showErrors(
																labelField.state.meta
															) &&
																labelField.state.meta.errors
																	.length > 0}
														/>
														{#if showErrors(labelField.state.meta) && labelField.state.meta.errors[0]}
															<Field.FieldError
																>{labelField.state.meta
																	.errors[0]}</Field.FieldError
															>
														{/if}
													{/snippet}
												</form.Field>
												<form.Field
													name={`criteria[${index}].description`}
													validators={{
														onChange: ({ value }) =>
															!value.trim()
																? 'Description is required.'
																: undefined
													}}
												>
													{#snippet children(descriptionField)}
														<Textarea
															id={`criterion-description-${criterion.clientId}`}
															aria-label="Description"
															placeholder="Description"
															value={descriptionField.state.value}
															disabled={!editable}
															onblur={descriptionField.handleBlur}
															oninput={(event) =>
																descriptionField.handleChange(
																	event.currentTarget.value
																)}
															aria-invalid={showErrors(
																descriptionField.state.meta
															) &&
																descriptionField.state.meta.errors
																	.length > 0}
															class="min-h-16"
														/>
														{#if showErrors(descriptionField.state.meta) && descriptionField.state.meta.errors[0]}
															<Field.FieldError
																>{descriptionField.state.meta
																	.errors[0]}</Field.FieldError
															>
														{/if}
													{/snippet}
												</form.Field>
											</div>
										{:else}
											<Empty.Root class="border-dashed p-8">
												<Empty.Media variant="icon"
													><ListChecksIcon /></Empty.Media
												>
												<Empty.Header>
													<Empty.Title
														>No eligibility criteria yet</Empty.Title
													>
													<Empty.Description
														>Add the first inclusion or exclusion rule
														to make the protocol actionable.</Empty.Description
													>
												</Empty.Header>
											</Empty.Root>
										{/each}
										<Button
											id="protocol-add-criterion"
											type="button"
											variant="outline"
											class="w-fit"
											disabled={!editable}
											onclick={() =>
												criteriaField.pushValue({
													clientId: nextClientId('criterion'),
													kind: 'inclusion',
													stage: 'both',
													dimension: 'population',
													label: '',
													description: ''
												})}
										>
											<PlusIcon data-icon="inline-start" />Add criterion
										</Button>
									</div>
								{/snippet}
							</form.Field>
						</section>
					</Tabs.Content>
				</Tabs.Root>

				<form.Subscribe
					selector={(state) => ({
						version: state.values.version,
						isDirty: state.isDirty,
						isSubmitting: state.isSubmitting
					})}
				>
					{#snippet children(footerMeta)}
						<div
							class="sticky top-0 z-10 order-first -mx-4 -mt-4 flex flex-col gap-1.5 border-b bg-background px-4 py-3 sm:-mx-6 sm:-mt-6 sm:px-6 lg:-mx-8 lg:-mt-8 lg:px-8"
						>
							<div class="flex flex-wrap items-center justify-between gap-3">
								<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
									<span class="font-medium"
										>{amending
											? `Amending version ${footerMeta.version}`
											: `Draft · version ${footerMeta.version}`}</span
									>
									<span
										class="flex items-center gap-1.5 text-xs text-muted-foreground"
										aria-live="polite"
									>
										{#if isPending}
											<Spinner class="size-3.5" />
										{:else if footerMeta.isDirty || amending}
											<span
												class="size-2 rounded-full bg-warning"
												aria-hidden="true"
											></span>
										{:else if protocolId.current}
											<CheckCircle2Icon class="size-3.5" aria-hidden="true" />
										{/if}
										{statusLabel}
									</span>
									{#if restored}
										<span class="text-xs text-muted-foreground"
											>Restored unsaved changes ·
											<button
												type="button"
												class="underline underline-offset-2 hover:text-foreground"
												onclick={discardRestored}>Discard</button
											></span
										>
									{/if}
									{#if amending}<span class="text-xs text-muted-foreground"
											>Saving creates a new version; the published one stays
											unchanged.</span
										>{/if}
								</div>
								<div class="flex flex-wrap gap-2">
									{#if amending}<Button
											type="button"
											variant="ghost"
											disabled={isPending}
											onclick={() => void reconcileFromServer()}
											>Discard amendment</Button
										>{/if}
									<Button
										type="submit"
										variant="outline"
										disabled={!editable || isPending}
									>
										{#if saveProtocol.isPending || footerMeta.isSubmitting}
											<Spinner data-icon="inline-start" />
										{:else}
											<SaveIcon data-icon="inline-start" />
										{/if}
										Save draft
									</Button>
									<Button type="button" disabled={!canPublish} onclick={publish}>
										{#if publishProtocol.isPending}
											<Spinner data-icon="inline-start" />
										{/if}
										Publish version
									</Button>
								</div>
							</div>
							{#if issues.length > 0 && editable}
								<p
									class="flex flex-wrap items-center gap-x-2 text-xs {attempted
										? 'text-destructive'
										: 'text-muted-foreground'}"
									data-testid="protocol-issues"
								>
									<span>To publish:</span>
									{#each issues as issue, i (issue.message)}
										<button
											type="button"
											class="underline underline-offset-2 hover:text-foreground"
											onclick={() => void goToIssue(issue)}
											>{issue.message}</button
										>
										{#if i < issues.length - 1}<span aria-hidden="true">·</span
											>{/if}
									{/each}
								</p>
							{/if}
						</div>
					{/snippet}
				</form.Subscribe>
			</form>
		{/if}
	{/if}
</PageTemplate>
