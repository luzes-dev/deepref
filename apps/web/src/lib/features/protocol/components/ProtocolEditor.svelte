<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Card from '$lib/components/ui/card';
	import * as Field from '$lib/components/ui/field';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Textarea } from '$lib/components/ui/textarea';
	import * as Select from '$lib/components/ui/select';
	import ArrowDownIcon from '@lucide/svelte/icons/arrow-down';
	import ArrowUpIcon from '@lucide/svelte/icons/arrow-up';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SaveIcon from '@lucide/svelte/icons/save';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import { page } from '$app/state';
	import { useQueryClient } from '@tanstack/svelte-query';
	import {
		createGetProjectReviewProtocol,
		createPublishProjectReviewProtocol,
		createSaveProjectReviewProtocol,
		getGetProjectReviewProtocolQueryKey,
		isConflict,
		isNotFound,
		type ProtocolDto,
		type PublishProtocolRequest,
		type SaveProtocolRequest
	} from '../api';
	import {
		CRITERION_DIMENSIONS,
		CRITERION_KINDS,
		CRITERION_STAGES,
		FRAMEWORK_FIELDS,
		FRAMEWORK_KINDS,
		REQUIRED_FRAMEWORK_FIELDS,
		duplicateCustomKeys,
		frameworkFieldsForKind,
		humanizeKey,
		isCriterionDimension,
		isCriterionKind,
		isCriterionStage,
		isFrameworkKind,
		isRequiredFrameworkField,
		type CriterionDimension,
		type CriterionKind,
		type CriterionStage,
		type FrameworkKind
	} from '../codecs';

	type DraftCriterion = {
		clientId: string;
		id?: string;
		kind: CriterionKind;
		stage: CriterionStage;
		dimension: CriterionDimension;
		label: string;
		description: string;
	};

	type ProtocolDraft = {
		id?: string;
		version: number;
		status: 'draft' | 'published' | 'superseded';
		name: string;
		objective: string;
		question: string;
		frameworkKind: FrameworkKind;
		frameworkFields: Record<string, string>;
		customFrameworkFields: CustomFrameworkField[];
		frameworkFieldSnapshots: Partial<Record<FrameworkKind, Record<string, string>>>;
		customFrameworkSnapshot?: CustomFrameworkField[];
		criteria: DraftCriterion[];
		revision: number;
		amendmentOf?: string | null;
	};
	type CustomFrameworkField = { clientId: string; key: string; value: string };

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

	let draft = $state(emptyDraft());
	let hydratedKey = $state<string | undefined>(undefined);
	let amending = $state(false);
	let dirty = $state(false);
	let reconciling = $state(false);
	let clientCriterionId = 0;
	let clientFieldId = 0;

	const protocol = $derived(protocolQuery.data?.data);
	const notFound = $derived(isNotFound(protocolQuery.error));
	const isPublished = $derived(draft.status === 'published' && !amending);
	const editable = $derived(!isPublished && draft.status !== 'superseded');
	const frameworkFields = $derived(FRAMEWORK_FIELDS[draft.frameworkKind]);
	const validationErrors = $derived(validateDraft(draft));
	const errorMessage = $derived(
		saveProtocol.error?.message ??
			publishProtocol.error?.message ??
			(notFound ? undefined : protocolQuery.error?.message)
	);
	const conflict = $derived(isConflict(saveProtocol.error) || isConflict(publishProtocol.error));
	const isPending = $derived(saveProtocol.isPending || publishProtocol.isPending);
	const canSave = $derived(editable && !isPending && validationErrors.length === 0);
	const canPublish = $derived(
		draft.status === 'draft' && Boolean(draft.id) && !isPending && validationErrors.length === 0
	);

	$effect(() => {
		const current = protocol;
		const nextKey = current
			? `${current.id}:${current.revision}`
			: notFound
				? 'new'
				: undefined;
		if (reconciling || nextKey === undefined || nextKey === hydratedKey || (dirty && current))
			return;
		if (current && current.revision < draft.revision) return;
		if (current) {
			draft = fromProtocol(current);
		} else {
			draft = emptyDraft();
		}
		hydratedKey = nextKey;
		dirty = false;
		amending = false;
	});

	function emptyDraft(): ProtocolDraft {
		return {
			version: 1,
			status: 'draft',
			name: '',
			objective: '',
			question: '',
			frameworkKind: 'pico',
			frameworkFields: frameworkFieldsForKind('pico', {}),
			customFrameworkFields: [],
			frameworkFieldSnapshots: {},
			criteria: [],
			revision: 0
		};
	}

	function fromProtocol(value: ProtocolDto): ProtocolDraft {
		const kind = isFrameworkKind(value.framework_kind) ? value.framework_kind : 'custom';
		const frameworkFields = stringRecord(value.framework_fields);
		const knownFields = frameworkFieldsForKind(kind, frameworkFields);
		const customFrameworkFields: CustomFrameworkField[] =
			kind === 'custom'
				? Object.entries(frameworkFields).map(([key, fieldValue]) => ({
						clientId: `field-${clientFieldId++}`,
						key,
						value: fieldValue
					}))
				: [];
		return {
			id: value.id,
			version: value.version,
			status: normalizeStatus(value.status),
			name: value.name,
			objective: value.objective,
			question: value.question,
			frameworkKind: kind,
			frameworkFields: knownFields,
			customFrameworkFields,
			frameworkFieldSnapshots: kind === 'custom' ? {} : { [kind]: { ...knownFields } },
			customFrameworkSnapshot:
				kind === 'custom' ? cloneCustomFields(customFrameworkFields) : undefined,
			criteria: parseCriteria(value.criteria),
			revision: value.revision,
			amendmentOf: value.amendment_of
		};
	}

	function stringRecord(value: unknown): Record<string, string> {
		if (typeof value !== 'object' || value === null || Array.isArray(value)) return {};
		return Object.fromEntries(
			Object.entries(value).filter(
				(entry): entry is [string, string] => typeof entry[1] === 'string'
			)
		);
	}

	function parseCriteria(value: unknown): DraftCriterion[] {
		if (!Array.isArray(value)) return [];
		return value.flatMap((item): DraftCriterion[] => {
			if (typeof item !== 'object' || item === null) return [];
			const id = 'id' in item && typeof item.id === 'string' ? item.id : undefined;
			const kind =
				'kind' in item && typeof item.kind === 'string' && isCriterionKind(item.kind)
					? item.kind
					: undefined;
			const stage =
				'stage' in item && typeof item.stage === 'string' && isCriterionStage(item.stage)
					? item.stage
					: undefined;
			const dimension =
				'dimension' in item &&
				typeof item.dimension === 'string' &&
				isCriterionDimension(item.dimension)
					? item.dimension
					: undefined;
			const label = 'label' in item && typeof item.label === 'string' ? item.label : '';
			const description =
				'description' in item && typeof item.description === 'string'
					? item.description
					: '';
			if (!kind || !stage || !dimension) return [];
			return [
				{
					clientId: id ?? `criterion-${clientCriterionId++}`,
					id,
					kind,
					stage,
					dimension,
					label,
					description
				}
			];
		});
	}

	function normalizeStatus(value: string): ProtocolDraft['status'] {
		if (value === 'published' || value === 'superseded') return value;
		return 'draft';
	}

	function markDirty(): void {
		dirty = true;
	}

	function changeFramework(value: string | undefined): void {
		if (!value || !isFrameworkKind(value)) return;
		const previousKind = draft.frameworkKind;
		if (previousKind === 'custom') {
			draft.customFrameworkSnapshot = cloneCustomFields(draft.customFrameworkFields);
		} else {
			draft.frameworkFieldSnapshots[previousKind] = frameworkFieldsForKind(
				previousKind,
				draft.frameworkFields
			);
		}

		if (value === 'custom') {
			const customFields =
				draft.customFrameworkSnapshot ?? customFieldsFromRecord(draft.frameworkFields);
			draft.customFrameworkFields = cloneCustomFields(customFields);
			draft.frameworkFields = {};
		} else {
			const savedFields = draft.frameworkFieldSnapshots[value];
			draft.frameworkFields = frameworkFieldsForKind(
				value,
				savedFields ?? draft.frameworkFields
			);
			draft.customFrameworkFields = [];
		}
		draft.frameworkKind = value;
		markDirty();
	}

	function changeKnownField(field: string, value: string): void {
		draft.frameworkFields = frameworkFieldsForKind(draft.frameworkKind, {
			...draft.frameworkFields,
			[field]: value
		});
		markDirty();
	}

	function customFieldsFromRecord(
		values: Readonly<Record<string, string>>
	): CustomFrameworkField[] {
		return Object.entries(values).map(([key, value]) => ({
			clientId: `field-${clientFieldId++}`,
			key,
			value
		}));
	}

	function cloneCustomFields(
		fields: ReadonlyArray<CustomFrameworkField>
	): CustomFrameworkField[] {
		return fields.map((field) => ({ ...field }));
	}

	function addCustomField(): void {
		draft.customFrameworkFields.push({
			clientId: `field-${clientFieldId++}`,
			key: '',
			value: ''
		});
		markDirty();
	}

	function removeCustomField(clientId: string): void {
		draft.customFrameworkFields = draft.customFrameworkFields.filter(
			(field) => field.clientId !== clientId
		);
		markDirty();
	}

	function addCriterion(): void {
		draft.criteria.push({
			clientId: `criterion-${clientCriterionId++}`,
			kind: 'inclusion',
			stage: 'both',
			dimension: 'other',
			label: '',
			description: ''
		});
		markDirty();
	}

	function removeCriterion(clientId: string): void {
		draft.criteria = draft.criteria.filter((criterion) => criterion.clientId !== clientId);
		markDirty();
	}

	function moveCriterion(index: number, offset: -1 | 1): void {
		const nextIndex = index + offset;
		if (nextIndex < 0 || nextIndex >= draft.criteria.length) return;
		const current = draft.criteria[index];
		const next = draft.criteria[nextIndex];
		if (!current || !next) return;
		draft.criteria[index] = next;
		draft.criteria[nextIndex] = current;
		markDirty();
	}

	function frameworkPayload(): Record<string, string> {
		if (draft.frameworkKind !== 'custom')
			return frameworkFieldsForKind(draft.frameworkKind, draft.frameworkFields);
		const fields: Record<string, string> = {};
		for (const field of draft.customFrameworkFields) {
			const key = field.key.trim();
			if (key) fields[key] = field.value.trim();
		}
		return fields;
	}

	function saveRequest(): SaveProtocolRequest {
		const request: SaveProtocolRequest = {
			name: draft.name.trim(),
			objective: draft.objective.trim(),
			question: draft.question.trim(),
			framework: { kind: draft.frameworkKind, fields: frameworkPayload() },
			criteria: draft.criteria.map((criterion) => ({
				...(criterion.id ? { id: criterion.id } : {}),
				kind: criterion.kind,
				stage: criterion.stage,
				dimension: criterion.dimension,
				label: criterion.label.trim(),
				description: criterion.description.trim()
			})),
			protocol_version_id: draft.id,
			expected_revision: draft.revision
		};
		return request;
	}

	async function save(): Promise<void> {
		if (!canSave) return;
		try {
			const result = await saveProtocol.mutateAsync({ projectId, data: saveRequest() });
			draft = fromProtocol(result.data);
			hydratedKey = `${result.data.id}:${result.data.revision}`;
			dirty = false;
			amending = false;
			await queryClient.invalidateQueries({
				queryKey: getGetProjectReviewProtocolQueryKey(projectId)
			});
		} catch {
			// The mutation error and conflict recovery action are rendered below.
		}
	}

	async function publish(): Promise<void> {
		const protocolVersionId = draft.id;
		if (!canPublish || !protocolVersionId) return;
		const request: PublishProtocolRequest = {
			protocol_version_id: protocolVersionId,
			expected_revision: draft.revision
		};
		try {
			const result = await publishProtocol.mutateAsync({ projectId, data: request });
			draft = fromProtocol(result.data);
			hydratedKey = `${result.data.id}:${result.data.revision}`;
			dirty = false;
			await queryClient.invalidateQueries({
				queryKey: getGetProjectReviewProtocolQueryKey(projectId)
			});
		} catch {
			// The mutation error and conflict recovery action are rendered below.
		}
	}

	async function reconcileFromServer(): Promise<void> {
		saveProtocol.reset();
		publishProtocol.reset();
		dirty = false;
		amending = false;
		hydratedKey = undefined;
		reconciling = true;
		try {
			await protocolQuery.refetch();
		} finally {
			reconciling = false;
		}
	}

	function beginAmendment(): void {
		if (draft.status !== 'published') return;
		amending = true;
		dirty = true;
	}

	function validateDraft(value: ProtocolDraft): string[] {
		const errors: string[] = [];
		if (!value.name.trim()) errors.push('Give the protocol a name.');
		if (!value.objective.trim()) errors.push('Add the review objective.');
		if (!value.question.trim()) errors.push('Add the research question.');
		if (value.frameworkKind === 'custom') {
			const duplicateKeys = duplicateCustomKeys(value.customFrameworkFields);
			if (duplicateKeys.length > 0) {
				errors.push(
					`Custom framework field names must be unique: ${duplicateKeys.join(', ')}.`
				);
			}
			for (const field of value.customFrameworkFields) {
				if (!field.key.trim() || !field.value.trim()) {
					errors.push('Complete or remove every custom framework field.');
					break;
				}
			}
		} else {
			for (const field of REQUIRED_FRAMEWORK_FIELDS[value.frameworkKind]) {
				if (!value.frameworkFields[field]?.trim()) {
					errors.push(`Complete the required ${humanizeKey(field)} framework field.`);
				}
			}
		}
		for (const criterion of value.criteria) {
			if (!criterion.label.trim() || !criterion.description.trim()) {
				errors.push('Complete every eligibility criterion or remove it.');
				break;
			}
		}
		return errors;
	}
</script>

<svelte:head>
	<title>Protocol · DeepRef</title>
	<meta
		name="description"
		content="Versioned protocol and eligibility criteria for a DeepRef evidence workspace."
	/>
</svelte:head>

<div class="flex h-full min-h-0 flex-col gap-4 overflow-auto p-4 md:p-6">
	<header class="flex flex-wrap items-start justify-between gap-4">
		<div class="flex flex-col gap-2">
			<div class="flex items-center gap-2 text-sm text-muted-foreground">
				<ShieldCheckIcon />
				Evidence workspace / protocol
			</div>
			<h1 class="text-2xl font-semibold">Review protocol</h1>
			<p class="max-w-3xl text-sm text-muted-foreground">
				Define the scientific question and the ordered eligibility rules used by every
				screening decision.
			</p>
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<Badge variant="outline">v{draft.version}</Badge>
			<Badge variant={draft.status === 'published' ? 'default' : 'secondary'}>
				{draft.status}
			</Badge>
			{#if draft.amendmentOf}
				<Badge variant="outline">Amends {draft.amendmentOf.slice(0, 8)}</Badge>
			{/if}
		</div>
	</header>

	{#if errorMessage}
		<Alert.Root variant="destructive">
			<Alert.Title
				>{conflict ? 'Protocol changed elsewhere' : 'Protocol unavailable'}</Alert.Title
			>
			<Alert.Description>{errorMessage}</Alert.Description>
			{#if conflict}
				<Alert.Action onclick={() => void reconcileFromServer()}>
					<RefreshCwIcon data-icon="inline-start" />Refresh
				</Alert.Action>
			{/if}
		</Alert.Root>
	{:else if protocolQuery.isPending}
		<Card.Root>
			<Card.Content class="flex items-center gap-3 py-8">
				<Spinner /> Loading protocol…
			</Card.Content>
		</Card.Root>
	{:else if !notFound && !protocol}
		<Card.Root>
			<Card.Content class="flex flex-col gap-3 py-8">
				<p class="font-medium">Protocol could not be loaded.</p>
				<Button variant="outline" onclick={() => void protocolQuery.refetch()}>
					<RefreshCwIcon data-icon="inline-start" />Retry
				</Button>
			</Card.Content>
		</Card.Root>
	{:else}
		{#if isPublished}
			<Alert.Root>
				<ShieldCheckIcon />
				<Alert.Title>Published protocol is immutable</Alert.Title>
				<Alert.Description>
					Screening decisions remain tied to this exact version. Choose Amend to create a
					new draft without changing the published record.
				</Alert.Description>
				<Alert.Action onclick={beginAmendment}>Amend published version</Alert.Action>
			</Alert.Root>
		{:else if amending}
			<Alert.Root>
				<Alert.Title>Amendment draft</Alert.Title>
				<Alert.Description>
					Saving this draft sends the published version id and revision so the server can
					create a new immutable version.
				</Alert.Description>
			</Alert.Root>
		{/if}

		<div class="grid gap-4 xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]">
			<Card.Root>
				<Card.Header>
					<Card.Title>Research question</Card.Title>
					<Card.Description
						>Published text becomes part of the scientific artifact.</Card.Description
					>
				</Card.Header>
				<Card.Content>
					<Field.Group>
						<Field.Field data-invalid={!draft.name.trim()}>
							<Field.Label for="protocol-name">Name</Field.Label>
							<Input
								id="protocol-name"
								value={draft.name}
								oninput={(event) => {
									draft.name = event.currentTarget.value;
									markDirty();
								}}
								disabled={!editable}
								aria-invalid={!draft.name.trim()}
							/>
						</Field.Field>
						<Field.Field data-invalid={!draft.objective.trim()}>
							<Field.Label for="protocol-objective">Objective</Field.Label>
							<Textarea
								id="protocol-objective"
								value={draft.objective}
								oninput={(event) => {
									draft.objective = event.currentTarget.value;
									markDirty();
								}}
								class="min-h-28"
								disabled={!editable}
								aria-invalid={!draft.objective.trim()}
							/>
						</Field.Field>
						<Field.Field data-invalid={!draft.question.trim()}>
							<Field.Label for="protocol-question">Question</Field.Label>
							<Textarea
								id="protocol-question"
								value={draft.question}
								oninput={(event) => {
									draft.question = event.currentTarget.value;
									markDirty();
								}}
								class="min-h-28"
								disabled={!editable}
								aria-invalid={!draft.question.trim()}
							/>
						</Field.Field>
					</Field.Group>
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Framework</Card.Title>
					<Card.Description
						>Choose a structured framework or define your own fields.</Card.Description
					>
				</Card.Header>
				<Card.Content class="flex flex-col gap-4">
					<Field.Field>
						<Field.Label>Framework</Field.Label>
						<Select.Root
							type="single"
							value={draft.frameworkKind}
							onValueChange={changeFramework}
						>
							<Select.Trigger disabled={!editable}
								>{humanizeKey(draft.frameworkKind)}</Select.Trigger
							>
							<Select.Content>
								<Select.Group>
									{#each FRAMEWORK_KINDS as kind (kind)}
										<Select.Item value={kind} label={humanizeKey(kind)} />
									{/each}
								</Select.Group>
							</Select.Content>
						</Select.Root>
					</Field.Field>
					{#if draft.frameworkKind === 'custom'}
						<div class="flex flex-col gap-3">
							{#each draft.customFrameworkFields as field (field.clientId)}
								<div
									class="grid gap-2 sm:grid-cols-[minmax(0,0.8fr)_minmax(0,1fr)_auto]"
								>
									<Input
										aria-label={`Custom framework field ${field.clientId} name`}
										value={field.key}
										placeholder="Field name"
										disabled={!editable}
										oninput={(event) => {
											field.key = event.currentTarget.value;
											markDirty();
										}}
									/>
									<Input
										aria-label={`Custom framework field ${field.clientId} definition`}
										value={field.value}
										placeholder="Definition"
										disabled={!editable}
										oninput={(event) => {
											field.value = event.currentTarget.value;
											markDirty();
										}}
									/>
									<Button
										variant="ghost"
										size="icon"
										aria-label="Remove framework field"
										disabled={!editable}
										onclick={() => removeCustomField(field.clientId)}
									>
										<Trash2Icon />
									</Button>
								</div>
							{:else}
								<p class="text-sm text-muted-foreground">
									Add fields to describe your custom framework.
								</p>
							{/each}
							<Button
								variant="outline"
								class="w-fit"
								disabled={!editable}
								onclick={addCustomField}
							>
								<PlusIcon data-icon="inline-start" />Add field
							</Button>
						</div>
					{:else}
						<Field.Group>
							{#each frameworkFields as field (field)}
								<Field.Field
									data-invalid={isRequiredFrameworkField(
										draft.frameworkKind,
										field
									) && !draft.frameworkFields[field]?.trim()}
								>
									<Field.Label for={`framework-${field}`}
										>{humanizeKey(field)}</Field.Label
									>
									<Textarea
										id={`framework-${field}`}
										value={draft.frameworkFields[field] ?? ''}
										disabled={!editable}
										oninput={(event) =>
											changeKnownField(field, event.currentTarget.value)}
										class="min-h-20"
									/>
								</Field.Field>
							{/each}
						</Field.Group>
					{/if}
				</Card.Content>
			</Card.Root>
		</div>

		<Card.Root>
			<Card.Header class="flex-row items-start justify-between gap-3">
				<div>
					<Card.Title>Eligibility criteria</Card.Title>
					<Card.Description
						>Ordered inclusion and exclusion rules for each screening stage.</Card.Description
					>
				</div>
				<Button variant="outline" disabled={!editable} onclick={addCriterion}>
					<PlusIcon data-icon="inline-start" />Add criterion
				</Button>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4">
				{#each draft.criteria as criterion, index (criterion.clientId)}
					<div class="rounded-lg border p-4">
						<div class="mb-4 flex flex-wrap items-center justify-between gap-2">
							<Badge variant="outline">Criterion {index + 1}</Badge>
							<div class="flex items-center gap-1">
								<Button
									variant="ghost"
									size="icon"
									aria-label="Move criterion up"
									disabled={!editable || index === 0}
									onclick={() => moveCriterion(index, -1)}
								>
									<ArrowUpIcon />
								</Button>
								<Button
									variant="ghost"
									size="icon"
									aria-label="Move criterion down"
									disabled={!editable || index === draft.criteria.length - 1}
									onclick={() => moveCriterion(index, 1)}
								>
									<ArrowDownIcon />
								</Button>
								<Button
									variant="ghost"
									size="icon"
									aria-label="Remove criterion"
									disabled={!editable}
									onclick={() => removeCriterion(criterion.clientId)}
								>
									<Trash2Icon />
								</Button>
							</div>
						</div>
						<div class="grid gap-4 lg:grid-cols-3">
							<Field.Field>
								<Field.Label>Kind</Field.Label>
								<Select.Root
									type="single"
									value={criterion.kind}
									onValueChange={(value) => {
										if (value === 'inclusion' || value === 'exclusion')
											criterion.kind = value;
										markDirty();
									}}
								>
									<Select.Trigger disabled={!editable}
										>{humanizeKey(criterion.kind)}</Select.Trigger
									>
									<Select.Content
										><Select.Group
											>{#each CRITERION_KINDS as value (value)}<Select.Item
													{value}
													label={humanizeKey(value)}
												/>{/each}</Select.Group
										></Select.Content
									>
								</Select.Root>
							</Field.Field>
							<Field.Field>
								<Field.Label>Stage</Field.Label>
								<Select.Root
									type="single"
									value={criterion.stage}
									onValueChange={(value) => {
										if (
											value === 'title_abstract' ||
											value === 'full_text' ||
											value === 'both'
										)
											criterion.stage = value;
										markDirty();
									}}
								>
									<Select.Trigger disabled={!editable}
										>{humanizeKey(criterion.stage)}</Select.Trigger
									>
									<Select.Content
										><Select.Group
											>{#each CRITERION_STAGES as value (value)}<Select.Item
													{value}
													label={humanizeKey(value)}
												/>{/each}</Select.Group
										></Select.Content
									>
								</Select.Root>
							</Field.Field>
							<Field.Field>
								<Field.Label>Dimension</Field.Label>
								<Select.Root
									type="single"
									value={criterion.dimension}
									onValueChange={(value) => {
										if (value && isCriterionDimension(value))
											criterion.dimension = value;
										markDirty();
									}}
								>
									<Select.Trigger disabled={!editable}
										>{humanizeKey(criterion.dimension)}</Select.Trigger
									>
									<Select.Content
										><Select.Group
											>{#each CRITERION_DIMENSIONS as value (value)}<Select.Item
													{value}
													label={humanizeKey(value)}
												/>{/each}</Select.Group
										></Select.Content
									>
								</Select.Root>
							</Field.Field>
						</div>
						<Field.Group class="mt-4">
							<Field.Field data-invalid={!criterion.label.trim()}>
								<Field.Label for={`criterion-label-${criterion.clientId}`}
									>Label</Field.Label
								>
								<Input
									id={`criterion-label-${criterion.clientId}`}
									value={criterion.label}
									disabled={!editable}
									oninput={(event) => {
										criterion.label = event.currentTarget.value;
										markDirty();
									}}
									aria-invalid={!criterion.label.trim()}
								/>
							</Field.Field>
							<Field.Field data-invalid={!criterion.description.trim()}>
								<Field.Label for={`criterion-description-${criterion.clientId}`}
									>Description</Field.Label
								>
								<Textarea
									id={`criterion-description-${criterion.clientId}`}
									value={criterion.description}
									disabled={!editable}
									oninput={(event) => {
										criterion.description = event.currentTarget.value;
										markDirty();
									}}
									aria-invalid={!criterion.description.trim()}
									class="min-h-24"
								/>
							</Field.Field>
						</Field.Group>
					</div>
				{:else}
					<p
						class="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground"
					>
						No eligibility criteria yet. Add the first inclusion or exclusion rule.
					</p>
				{/each}
			</Card.Content>
		</Card.Root>

		{#if validationErrors.length > 0}
			<Alert.Root variant="destructive">
				<Alert.Title>Complete the protocol before saving</Alert.Title>
				<Alert.Description>{validationErrors[0]}</Alert.Description>
			</Alert.Root>
		{/if}

		<div class="flex flex-wrap justify-end gap-2 border-t pt-4">
			{#if isPublished}
				<Button variant="outline" onclick={beginAmendment}>Amend published version</Button>
			{:else}
				<Button variant="outline" disabled={!canSave} onclick={save}>
					{#if saveProtocol.isPending}<Spinner data-icon="inline-start" />{:else}<SaveIcon
							data-icon="inline-start"
						/>{/if}
					Save draft
				</Button>
				<Button disabled={!canPublish} onclick={publish}>
					{#if publishProtocol.isPending}<Spinner data-icon="inline-start" />{/if}
					Publish version
				</Button>
			{/if}
		</div>
	{/if}
</div>
