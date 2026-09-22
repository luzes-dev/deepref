<script lang="ts">
	import * as Tabs from '@deepref/ui/tabs';
	import * as Alert from '@deepref/ui/alert';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import * as Empty from '@deepref/ui/empty';
	import * as Field from '@deepref/ui/field';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Spinner } from '@deepref/ui/spinner';
	import { Textarea } from '@deepref/ui/textarea';
	import * as Select from '@deepref/ui/select';
	import { notifyError } from '$lib/features/notifications/toast';
	import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
	import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SaveIcon from '@lucide/svelte/icons/save';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import CheckCircle2Icon from '@lucide/svelte/icons/circle-check';
	import LockKeyholeIcon from '@lucide/svelte/icons/lock-keyhole';
	import ListChecksIcon from '@lucide/svelte/icons/list-checks';
	import { page } from '$app/state';
	import { createForm } from '@tanstack/svelte-form';
	import { useQueryClient } from '@tanstack/svelte-query';
	import {
		createGetProjectReviewProtocol,
		createPublishProjectReviewProtocol,
		createSaveProjectReviewProtocol,
		getGetProjectReviewProtocolQueryKey,
		isConflict,
		isNotFound,
		type PublishProtocolRequest
	} from '../api';
	import {
		CRITERION_DIMENSIONS,
		CRITERION_KINDS,
		CRITERION_STAGES,
		FRAMEWORK_FIELDS,
		FRAMEWORK_KINDS,
		frameworkFieldsForKind,
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
		protocolDraftFromDto,
		validateProtocolDraft,
		type DraftClientIdFactory
	} from '../draft';

	const projectId = $derived(page.params.projectId ?? '');
	const queryClient = useQueryClient();
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

	const protocol = $derived(protocolQuery.data?.data);
	const notFound = $derived(isNotFound(protocolQuery.error));

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
		validators: {
			onChange: ({ value }) => validateProtocolDraft(value)[0]
		},
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

	const isPublished = $derived(status.current === 'published' && !amending);
	const editable = $derived(!isPublished && status.current !== 'superseded');
	const errorMessage = $derived(notFound ? undefined : protocolQuery.error?.message);
	const isPending = $derived(
		saveProtocol.isPending || publishProtocol.isPending || isSubmitting.current
	);
	const canSave = $derived(editable && !isPending && formReady.current);
	const canPublish = $derived(
		status.current === 'draft' && Boolean(protocolId.current) && !isPending && formReady.current
	);

	$effect(() => {
		const current = protocol;
		const nextKey = current
			? `${current.id}:${current.revision}`
			: notFound
				? 'new'
				: undefined;
		if (
			reconciling ||
			nextKey === undefined ||
			nextKey === hydratedKey ||
			(isDirty.current && current && !forceHydrate)
		)
			return;
		if (current && current.revision < form.state.values.revision) return;
		form.reset(current ? protocolDraftFromDto(current, nextClientId) : emptyProtocolDraft());
		hydratedKey = nextKey;
		forceHydrate = false;
		amending = false;
	});

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
		try {
			const result = await saveProtocol.mutateAsync({
				projectId,
				data: buildSaveProtocolRequest(value)
			});
			form.reset(protocolDraftFromDto(result.data, nextClientId));
			hydratedKey = `${result.data.id}:${result.data.revision}`;
			amending = false;
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
		if (!canSave) return;
		await form.handleSubmit();
	}

	async function publish(): Promise<void> {
		const protocolVersionId = form.state.values.id;
		if (!canPublish || !protocolVersionId) return;
		const request: PublishProtocolRequest = {
			protocol_version_id: protocolVersionId,
			expected_revision: form.state.values.revision
		};
		try {
			const result = await publishProtocol.mutateAsync({ projectId, data: request });
			form.reset(protocolDraftFromDto(result.data, nextClientId));
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
	<form.Subscribe
		selector={(state) => ({
			version: state.values.version,
			status: state.values.status,
			amendmentOf: state.values.amendmentOf
		})}
	>
		{#snippet children(headerMeta)}
			<div class="flex flex-wrap items-center justify-end gap-2 border-b pb-3">
					<Badge variant="outline">v{headerMeta.version}</Badge>
					<Badge variant={headerMeta.status === 'published' ? 'default' : 'secondary'}>
						{headerMeta.status}
					</Badge>
					{#if headerMeta.amendmentOf}
						<Badge variant="outline">Amends {headerMeta.amendmentOf.slice(0, 8)}</Badge>
					{/if}
				</div>
		{/snippet}
	</form.Subscribe>

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
	{:else if !notFound && !protocol}
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
		{#if isPublished}
			<Alert.Root>
				<LockKeyholeIcon />
				<Alert.Title>Published protocol is immutable</Alert.Title>
				<Alert.Description>
					Screening decisions remain tied to this exact version. Choose Amend to create a
					new draft without changing the published record.
				</Alert.Description>
				<Alert.Action
					><Button type="button" variant="outline" onclick={beginAmendment}
						>Amend published version</Button
					></Alert.Action
				>
			</Alert.Root>
		{:else if amending}
			<Alert.Root class="border-primary/20 bg-primary/5">
				<BookOpenIcon />
				<Alert.Title>Amendment draft</Alert.Title>
				<Alert.Description>
					Saving this draft sends the published version id and revision so the server can
					create a new immutable version.
				</Alert.Description>
			</Alert.Root>
		{/if}

		<form
			class="flex min-w-0 flex-col gap-6"
			onsubmit={(event) => {
				event.preventDefault();
				event.stopPropagation();
				void save();
			}}
		>
			<Tabs.Root value="question" class="min-w-0 gap-6">
				<Tabs.List
					variant="line"
					aria-label="Page sections"
					class="max-w-full justify-start overflow-x-auto border-b"
					><Tabs.Trigger value="question">Research question</Tabs.Trigger><Tabs.Trigger
						value="framework">Framework</Tabs.Trigger
					><Tabs.Trigger value="criteria">Eligibility criteria</Tabs.Trigger></Tabs.List
				>
				<Tabs.Content value="question"
					><section class="workflow-section border-primary/15">
						<header class="flex flex-col gap-2 border-b border-border/60 pb-4">
							<div class="flex items-center gap-2">
								<span
									class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
									><BookOpenIcon aria-hidden="true" /></span
								>
								<h2 class="text-base font-semibold">Research question</h2>
							</div>
							<p class="text-sm text-muted-foreground">
								Published text becomes part of the scientific artifact.
							</p>
						</header>
						<div class="min-w-0 pt-5">
							<Field.Group>
								<form.Field
									name="name"
									validators={{
										onChange: ({ value }) =>
											!value.trim() ? 'Give the protocol a name.' : undefined,
										onChangeAsync: ({ value, signal }) =>
											validateUniqueProtocolName(value, signal),
										onChangeAsyncDebounceMs: 350
									}}
								>
									{#snippet children(field)}
										<Field.Field
											data-invalid={field.state.meta.errors.length > 0}
										>
											<Field.Label for="protocol-name">Name</Field.Label>
											<Input
												id="protocol-name"
												name={field.name}
												value={field.state.value}
												onblur={field.handleBlur}
												oninput={(event) =>
													field.handleChange(event.currentTarget.value)}
												disabled={!editable}
												aria-invalid={field.state.meta.errors.length > 0}
												aria-busy={field.state.meta.isValidating}
											/>
											{#if field.state.meta.errors[0]}<Field.FieldError
													>{field.state.meta.errors[0]}</Field.FieldError
												>{/if}
										</Field.Field>
									{/snippet}
								</form.Field>
								<form.Field
									name="objective"
									validators={{
										onChange: ({ value }) =>
											!value.trim() ? 'Add the review objective.' : undefined
									}}
								>
									{#snippet children(field)}
										<Field.Field
											data-invalid={field.state.meta.errors.length > 0}
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
													field.handleChange(event.currentTarget.value)}
												class="min-h-28"
												disabled={!editable}
												aria-invalid={field.state.meta.errors.length > 0}
											/>
											{#if field.state.meta.errors[0]}<Field.FieldError
													>{field.state.meta.errors[0]}</Field.FieldError
												>{/if}
										</Field.Field>
									{/snippet}
								</form.Field>
								<form.Field
									name="question"
									validators={{
										onChange: ({ value }) =>
											!value.trim() ? 'Add the research question.' : undefined
									}}
								>
									{#snippet children(field)}
										<Field.Field
											data-invalid={field.state.meta.errors.length > 0}
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
													field.handleChange(event.currentTarget.value)}
												class="min-h-28"
												disabled={!editable}
												aria-invalid={field.state.meta.errors.length > 0}
											/>
											{#if field.state.meta.errors[0]}<Field.FieldError
													>{field.state.meta.errors[0]}</Field.FieldError
												>{/if}
										</Field.Field>
									{/snippet}
								</form.Field>
							</Field.Group>
						</div>
					</section></Tabs.Content
				>
				<Tabs.Content value="framework"
					><section class="workflow-section border-primary/15">
						<header class="flex flex-col gap-2 border-b border-border/60 pb-4">
							<div class="flex items-center gap-2">
								<span
									class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
									><ListChecksIcon aria-hidden="true" /></span
								>
								<h2 class="text-base font-semibold">Framework</h2>
							</div>
							<p class="text-sm text-muted-foreground">
								Choose a structured framework or define your own fields.
							</p>
						</header>
						<div class="flex min-w-0 flex-col gap-4 pt-5">
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
												>{humanizeKey(field.state.value)}</Select.Trigger
											>
											<Select.Content>
												<Select.Group>
													{#each FRAMEWORK_KINDS as kind (kind)}
														<Select.Item
															value={kind}
															label={humanizeKey(kind)}
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
																		event.currentTarget.value
																	)}
																aria-invalid={keyField.state.meta
																	.errors.length > 0}
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
																		event.currentTarget.value
																	)}
																aria-invalid={valueField.state.meta
																	.errors.length > 0}
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
													data-invalid={ff.state.meta.errors.length > 0}
												>
													<Field.Label for={`framework-field-${field}`}
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
														aria-invalid={ff.state.meta.errors.length >
															0}
													/>
													{#if ff.state.meta.errors[0]}<Field.FieldError
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
					</section></Tabs.Content
				>
				<Tabs.Content value="criteria"
					><section class="workflow-section border-primary/15">
						<header
							class="flex flex-col gap-3 border-b border-border/60 pb-4 sm:flex-row sm:items-center sm:justify-between"
						>
							<div class="flex flex-col gap-1">
								<h2 class="text-base font-semibold">Eligibility criteria</h2>
								<p class="text-sm text-muted-foreground">
									Ordered rules evaluated during screening.
								</p>
							</div>
							<form.Field name="criteria" mode="array">
								{#snippet children(criteriaField)}
									<Button
										type="button"
										variant="outline"
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
								{/snippet}
							</form.Field>
						</header>

						<form.Field name="criteria" mode="array">
							{#snippet children(criteriaField)}
								<div class="flex flex-col gap-4 pt-5">
									{#each criteriaField.state.value as criterion, index (criterion.clientId)}
										<div
											class="flex flex-col gap-4 rounded-xl border border-border/70 p-4 transition-colors hover:border-border"
										>
											<div
												class="flex flex-wrap items-center justify-between gap-2"
											>
												<span
													class="font-mono text-xs font-medium text-muted-foreground"
												>
													#{index + 1}
												</span>
												<div class="flex items-center gap-1">
													<Button
														type="button"
														variant="ghost"
														size="icon"
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
														size="icon"
														aria-label="Move criterion down"
														disabled={!editable ||
															index ===
																criteriaField.state.value.length -
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
														size="icon"
														aria-label="Remove criterion"
														disabled={!editable}
														onclick={() =>
															criteriaField.removeValue(index)}
													>
														<Trash2Icon />
													</Button>
												</div>
											</div>
											<div class="grid gap-3 sm:grid-cols-3">
												<form.Field name={`criteria[${index}].kind`}>
													{#snippet children(kindField)}
														<Field.Field>
															<Field.Label
																for={`criterion-kind-${criterion.clientId}`}
																>Type</Field.Label
															>
															<Select.Root
																type="single"
																value={kindField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionKind(value)
																	) {
																		kindField.handleChange(
																			value
																		);
																	}
																}}
															>
																<Select.Trigger
																	id={`criterion-kind-${criterion.clientId}`}
																	disabled={!editable}
																	>{humanizeKey(
																		kindField.state.value
																	)}</Select.Trigger
																>
																<Select.Content>
																	<Select.Group>
																		{#each CRITERION_KINDS as kind (kind)}
																			<Select.Item
																				value={kind}
																				label={humanizeKey(
																					kind
																				)}
																			/>
																		{/each}
																	</Select.Group>
																</Select.Content>
															</Select.Root>
														</Field.Field>
													{/snippet}
												</form.Field>
												<form.Field name={`criteria[${index}].stage`}>
													{#snippet children(stageField)}
														<Field.Field>
															<Field.Label
																for={`criterion-stage-${criterion.clientId}`}
																>Screening stage</Field.Label
															>
															<Select.Root
																type="single"
																value={stageField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionStage(value)
																	) {
																		stageField.handleChange(
																			value
																		);
																	}
																}}
															>
																<Select.Trigger
																	id={`criterion-stage-${criterion.clientId}`}
																	disabled={!editable}
																	>{humanizeKey(
																		stageField.state.value
																	)}</Select.Trigger
																>
																<Select.Content>
																	<Select.Group>
																		{#each CRITERION_STAGES as stage (stage)}
																			<Select.Item
																				value={stage}
																				label={humanizeKey(
																					stage
																				)}
																			/>
																		{/each}
																	</Select.Group>
																</Select.Content>
															</Select.Root>
														</Field.Field>
													{/snippet}
												</form.Field>
												<form.Field name={`criteria[${index}].dimension`}>
													{#snippet children(dimField)}
														<Field.Field>
															<Field.Label
																for={`criterion-dim-${criterion.clientId}`}
																>PICO domain</Field.Label
															>
															<Select.Root
																type="single"
																value={dimField.state.value}
																onValueChange={(value) => {
																	if (
																		value &&
																		isCriterionDimension(value)
																	) {
																		dimField.handleChange(
																			value
																		);
																	}
																}}
															>
																<Select.Trigger
																	id={`criterion-dim-${criterion.clientId}`}
																	disabled={!editable}
																	>{humanizeKey(
																		dimField.state.value
																	)}</Select.Trigger
																>
																<Select.Content>
																	<Select.Group>
																		{#each CRITERION_DIMENSIONS as dim (dim)}
																			<Select.Item
																				value={dim}
																				label={humanizeKey(
																					dim
																				)}
																			/>
																		{/each}
																	</Select.Group>
																</Select.Content>
															</Select.Root>
														</Field.Field>
													{/snippet}
												</form.Field>
											</div>
											<Field.Group>
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
														<Field.Field
															data-invalid={labelField.state.meta
																.errors.length > 0}
														>
															<Field.Label
																for={`criterion-label-${criterion.clientId}`}
																>Label</Field.Label
															>
															<Input
																id={`criterion-label-${criterion.clientId}`}
																value={labelField.state.value}
																disabled={!editable}
																onblur={labelField.handleBlur}
																oninput={(event) =>
																	labelField.handleChange(
																		event.currentTarget.value
																	)}
																aria-invalid={labelField.state.meta
																	.errors.length > 0}
															/>
															{#if labelField.state.meta.errors[0]}<Field.FieldError
																	>{labelField.state.meta
																		.errors[0]}</Field.FieldError
																>{/if}
														</Field.Field>
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
														<Field.Field
															data-invalid={descriptionField.state
																.meta.errors.length > 0}
														>
															<Field.Label
																for={`criterion-description-${criterion.clientId}`}
																>Description</Field.Label
															>
															<Textarea
																id={`criterion-description-${criterion.clientId}`}
																value={descriptionField.state.value}
																disabled={!editable}
																onblur={descriptionField.handleBlur}
																oninput={(event) =>
																	descriptionField.handleChange(
																		event.currentTarget.value
																	)}
																aria-invalid={descriptionField.state
																	.meta.errors.length > 0}
																class="min-h-24"
															/>
															{#if descriptionField.state.meta.errors[0]}<Field.FieldError
																	>{descriptionField.state.meta
																		.errors[0]}</Field.FieldError
																>{/if}
														</Field.Field>
													{/snippet}
												</form.Field>
											</Field.Group>
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
													>Add the first inclusion or exclusion rule to
													make the protocol actionable.</Empty.Description
												>
											</Empty.Header>
										</Empty.Root>
									{/each}
								</div>
							{/snippet}
						</form.Field>
					</section></Tabs.Content
				>
			</Tabs.Root>

			<form.Subscribe
				selector={(state) =>
					(state.errors as (string | undefined)[]).find(
						(error): error is string => typeof error === 'string'
					)}
			>
				{#snippet children(firstError)}
					{#if firstError}
						<Alert.Root variant="destructive">
							<Alert.Title>Complete the protocol before saving</Alert.Title>
							<Alert.Description>{firstError}</Alert.Description>
						</Alert.Root>
					{/if}
				{/snippet}
			</form.Subscribe>

			<form.Subscribe
				selector={(state) => ({
					isDirty: state.isDirty,
					isSubmitting: state.isSubmitting,
					formReady:
						!state.isValidating &&
						state.isValid &&
						validateProtocolDraft(state.values).length === 0
				})}
			>
				{#snippet children(footerMeta)}
					<div
						class="flex flex-wrap items-center justify-between gap-3 border-t border-border/60 pt-4"
					>
						<div class="flex items-center gap-2 text-xs text-muted-foreground">
							{#if footerMeta.isDirty}
								<span class="size-2 rounded-full bg-warning" aria-hidden="true"
								></span> Unsaved changes
							{:else}
								<CheckCircle2Icon aria-hidden="true" /> Draft is saved
							{/if}
						</div>
						<div class="flex flex-wrap justify-end gap-2">
							{#if isPublished}
								<Button type="button" variant="outline" onclick={beginAmendment}
									>Amend published version</Button
								>
							{:else}
								<Button
									type="submit"
									variant="outline"
									disabled={!editable || isPending || !footerMeta.formReady}
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
							{/if}
						</div>
					</div>
				{/snippet}
			</form.Subscribe>
		</form>
	{/if}
</PageTemplate>
