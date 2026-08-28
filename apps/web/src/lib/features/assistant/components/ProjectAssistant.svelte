<script lang="ts">
	import { resolve } from '$app/paths';
	import type { Pathname } from '$app/types';
	import {
		createExecuteProjectAssistantTool,
		createListProjectAssistantTools
	} from '$lib/api/generated/assistant/assistant';
	import type { AssistantToolResponse, AssistantToolDescriptor } from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import { AssistantToolKind } from '$lib/api/generated/models';
	import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
	import FileSearchIcon from '@lucide/svelte/icons/file-search';
	import FlaskConicalIcon from '@lucide/svelte/icons/flask-conical';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SendIcon from '@lucide/svelte/icons/send';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Textarea } from '$lib/components/ui/textarea';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import {
		ASSISTANT_TOOL_METADATA,
		initialToolValues,
		partitionAssistantCatalog,
		reviewPath,
		serializeToolRequest,
		type SupportedCatalogEntry,
		type ToolValidation,
		type ToolField,
		type ToolName,
		type ToolValues
	} from '../tools';

	let { projectId }: { projectId: string } = $props();

	const ACTOR_HEADERS = {
		'x-actor-kind': 'user',
		'x-actor-id': 'local-user'
	} satisfies Record<string, string>;

	const catalogQuery = createListProjectAssistantTools(() => projectId);
	const executeMutation = createExecuteProjectAssistantTool(() => ({
		request: { headers: ACTOR_HEADERS }
	}));

	let selectedToolName = $state<ToolName | null>(null);
	let values = $state<ToolValues>({});
	let assistantResult = $state<AssistantToolResponse | null>(null);
	let lastRequest = $state<Extract<ToolValidation, { kind: 'valid' }>['request'] | null>(null);
	const EMPTY_ERRORS: Readonly<Record<string, string>> = {};

	const catalog = $derived(catalogQuery.data?.data ?? []);
	const catalogPartition = $derived(partitionAssistantCatalog(catalog));
	const selectedEntry = $derived.by(() =>
		selectedToolName
			? catalogPartition.supported.find((entry) => entry.metadata.name === selectedToolName)
			: undefined
	);
	const readTools = $derived(
		catalogPartition.supported.filter((entry) => entry.metadata.kind === AssistantToolKind.read)
	);
	const proposalTools = $derived(
		catalogPartition.supported.filter(
			(entry) => entry.metadata.kind === AssistantToolKind.proposal
		)
	);
	const validation = $derived(
		selectedToolName ? serializeToolRequest(selectedToolName, projectId, values) : null
	);
	const validationErrors = $derived(
		validation?.kind === 'invalid' ? validation.errors : EMPTY_ERRORS
	);
	const reviewHref = $derived(
		selectedToolName && ASSISTANT_TOOL_METADATA[selectedToolName].reviewDestination
			? reviewPath(selectedToolName, projectId, values)
			: null
	);

	function selectTool(entry: SupportedCatalogEntry): void {
		selectedToolName = entry.metadata.name;
		values = initialToolValues(entry.metadata.name);
		assistantResult = null;
		lastRequest = null;
		executeMutation.reset();
	}

	function updateField(key: string, event: Event): void {
		const target = event.currentTarget;
		if (!(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement)) return;
		values = { ...values, [key]: target.value };
		assistantResult = null;
	}

	function updateStage(value: string | undefined): void {
		if (value !== 'title_abstract' && value !== 'full_text') return;
		values = { ...values, stage: value };
		assistantResult = null;
	}

	async function execute(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		if (!selectedToolName || !validation || validation.kind !== 'valid') return;
		if (executeMutation.isPending) return;

		assistantResult = null;
		lastRequest = validation.request;
		try {
			const response = await executeMutation.mutateAsync({
				projectId,
				data: validation.request
			});
			assistantResult = response.data;
		} catch {
			// The bounded API error is rendered below.
		}
	}

	async function retryExecution(): Promise<void> {
		if (!lastRequest || executeMutation.isPending) return;
		if (lastRequest.args.project_id !== projectId) return;
		try {
			const response = await executeMutation.mutateAsync({
				projectId,
				data: lastRequest
			});
			assistantResult = response.data;
		} catch {
			// The bounded API error is rendered below.
		}
	}

	async function retryCatalog(): Promise<void> {
		await catalogQuery.refetch();
	}

	function fieldValue(field: ToolField): string {
		return values[field.key] ?? '';
	}

	function executionError(error: unknown): { title: string; message: string; retry: boolean } {
		if (!(error instanceof ApiError)) {
			return {
				title: 'Assistant request failed',
				message: 'The assistant could not complete this request.',
				retry: false
			};
		}

		switch (error.status) {
			case 400:
				return { title: 'Request rejected', message: error.message, retry: false };
			case 403:
				return { title: 'Permission denied', message: error.message, retry: false };
			case 404:
				return { title: 'Resource not found', message: error.message, retry: false };
			case 409:
				return { title: 'Proposal conflict', message: error.message, retry: false };
			case 503:
				return { title: 'AI provider unavailable', message: error.message, retry: true };
			default:
				return { title: 'Assistant request failed', message: error.message, retry: false };
		}
	}

	function catalogErrorMessage(error: unknown): string {
		return error instanceof ApiError
			? error.message
			: 'The project assistant tool catalog could not be loaded.';
	}

	function readResult(data: Record<string, unknown>): string {
		return JSON.stringify(data, null, 2) ?? '{}';
	}

	function fieldId(field: ToolField): string {
		return `assistant-${field.key}`;
	}

	function toolDescription(entry: SupportedCatalogEntry): string {
		return entry.descriptor.description || entry.metadata.description;
	}

	function unsupportedName(descriptor: AssistantToolDescriptor): string {
		return descriptor.name || 'Unnamed server tool';
	}
</script>

<svelte:head>
	<title>Project Assistant · DeepRef</title>
	<meta
		name="description"
		content="Run one explicitly selected, policy-governed project assistant tool at a time."
	/>
</svelte:head>

<main class="mx-auto flex w-full max-w-7xl flex-col gap-6 p-4 md:p-8">
	<header class="flex flex-col gap-3">
		<div class="flex flex-wrap items-center gap-2">
			<Badge variant="secondary">Guided tools</Badge>
			<Badge variant="outline">Project scoped</Badge>
		</div>
		<div>
			<h1 class="text-2xl font-semibold tracking-tight">Project Assistant</h1>
			<p class="mt-1 max-w-3xl text-sm text-muted-foreground">
				Select one approved read or proposal tool, provide its typed inputs, and run exactly
				one request. Proposal tools create reviewer work; they never change scientific state
				directly.
			</p>
		</div>
	</header>

	{#if catalogQuery.isPending}
		<section class="grid gap-4 md:grid-cols-2" aria-label="Loading assistant tools">
			{#each [0, 1, 2, 3] as index (index)}
				<Skeleton class="h-24 w-full" />
			{/each}
		</section>
	{:else if catalogQuery.error}
		<Alert.Root variant="destructive" data-testid="assistant-catalog-error">
			<AlertCircleIcon data-icon="inline-start" />
			<Alert.Title>Could not load assistant tools</Alert.Title>
			<Alert.Description>{catalogErrorMessage(catalogQuery.error)}</Alert.Description>
			<Alert.Action>
				<Button
					variant="outline"
					size="sm"
					onclick={retryCatalog}
					data-testid="assistant-catalog-retry"
				>
					<RefreshCwIcon data-icon="inline-start" />
					Retry
				</Button>
			</Alert.Action>
		</Alert.Root>
	{:else if catalog.length === 0}
		<Empty.Root data-testid="assistant-catalog-empty">
			<Empty.Header>
				<Empty.Media variant="icon"><ShieldCheckIcon /></Empty.Media>
				<Empty.Title>No assistant tools are available</Empty.Title>
				<Empty.Description>
					This project did not return any policy-approved assistant tools.
				</Empty.Description>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<section class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)]">
			<div class="flex min-w-0 flex-col gap-4">
				<Card.Root>
					<Card.Header>
						<Card.Title>Choose a tool</Card.Title>
						<Card.Description>
							The server catalog is intersected with this exact client form catalog.
							Unknown tools remain visible as unsupported and cannot be executed.
						</Card.Description>
					</Card.Header>
					<Card.Content class="flex flex-col gap-5">
						<section class="flex flex-col gap-2" aria-labelledby="assistant-read-tools">
							<div class="flex items-center gap-2">
								<BookOpenIcon
									class="size-4 text-muted-foreground"
									aria-hidden={true}
								/>
								<h2 id="assistant-read-tools" class="text-sm font-medium">Reads</h2>
							</div>
							{#if readTools.length === 0}
								<p class="text-sm text-muted-foreground">
									No read tools are available.
								</p>
							{:else}
								<div class="grid gap-2">
									{#each readTools as entry (entry.metadata.name)}
										<Button
											variant={selectedToolName === entry.metadata.name
												? 'secondary'
												: 'outline'}
											class="h-auto justify-start px-3 py-2 text-left whitespace-normal"
											onclick={() => selectTool(entry)}
											data-testid={`assistant-tool-${entry.metadata.name}`}
										>
											<span class="flex min-w-0 flex-col items-start gap-1">
												<span class="font-medium"
													>{entry.metadata.label}</span
												>
												<span
													class="text-xs font-normal text-muted-foreground"
												>
													{toolDescription(entry)}
												</span>
											</span>
										</Button>
									{/each}
								</div>
							{/if}
						</section>

						<section
							class="flex flex-col gap-2"
							aria-labelledby="assistant-proposal-tools"
						>
							<div class="flex items-center gap-2">
								<FlaskConicalIcon
									class="size-4 text-muted-foreground"
									aria-hidden={true}
								/>
								<h2 id="assistant-proposal-tools" class="text-sm font-medium">
									Proposals
								</h2>
							</div>
							{#if proposalTools.length === 0}
								<p class="text-sm text-muted-foreground">
									No proposal tools are available.
								</p>
							{:else}
								<div class="grid gap-2">
									{#each proposalTools as entry (entry.metadata.name)}
										<Button
											variant={selectedToolName === entry.metadata.name
												? 'secondary'
												: 'outline'}
											class="h-auto justify-start px-3 py-2 text-left whitespace-normal"
											onclick={() => selectTool(entry)}
											data-testid={`assistant-tool-${entry.metadata.name}`}
										>
											<span class="flex min-w-0 flex-col items-start gap-1">
												<span class="font-medium"
													>{entry.metadata.label}</span
												>
												<span
													class="text-xs font-normal text-muted-foreground"
												>
													{toolDescription(entry)}
												</span>
											</span>
										</Button>
									{/each}
								</div>
							{/if}
						</section>

						{#if catalogPartition.unsupported.length > 0}
							<section
								class="flex flex-col gap-2"
								aria-labelledby="assistant-unsupported-tools"
							>
								<div class="flex items-center gap-2">
									<ShieldCheckIcon
										class="size-4 text-muted-foreground"
										aria-hidden={true}
									/>
									<h2
										id="assistant-unsupported-tools"
										class="text-sm font-medium"
									>
										Unsupported
									</h2>
								</div>
								<div
									class="flex flex-col gap-2"
									data-testid="assistant-unsupported-tools"
								>
									{#each catalogPartition.unsupported as descriptor (descriptor.name)}
										<div class="rounded-md border border-dashed p-3 text-sm">
											<div class="flex flex-wrap items-center gap-2">
												<span class="font-medium"
													>{unsupportedName(descriptor)}</span
												>
												<Badge variant="outline"
													>Not supported by this UI</Badge
												>
											</div>
											<p class="mt-1 text-xs text-muted-foreground">
												{descriptor.description ||
													'This server tool has no safe guided form.'}
											</p>
										</div>
									{/each}
								</div>
							</section>
						{/if}
					</Card.Content>
				</Card.Root>
			</div>

			<div class="min-w-0">
				{#if !selectedEntry || !selectedToolName}
					<Empty.Root class="min-h-64" data-testid="assistant-tool-empty">
						<Empty.Header>
							<Empty.Media variant="icon"><FileSearchIcon /></Empty.Media>
							<Empty.Title>Select a tool to begin</Empty.Title>
							<Empty.Description>
								Inputs are shown only after you explicitly choose one catalog entry.
							</Empty.Description>
						</Empty.Header>
					</Empty.Root>
				{:else}
					<Card.Root>
						<Card.Header>
							<div class="flex flex-wrap items-start justify-between gap-3">
								<div>
									<Card.Title>{selectedEntry.metadata.label}</Card.Title>
									<Card.Description
										>{selectedEntry.metadata.description}</Card.Description
									>
								</div>
								<Badge variant="outline"
									>{selectedEntry.descriptor.authority_tier}</Badge
								>
							</div>
						</Card.Header>
						<Card.Content>
							<form
								class="flex flex-col gap-5"
								onsubmit={execute}
								data-testid="assistant-tool-form"
							>
								<Field.FieldGroup>
									{#if selectedEntry.metadata.fields.length === 0}
										<p class="text-sm text-muted-foreground">
											This read uses the selected project scope and needs no
											additional inputs.
										</p>
									{/if}
									{#each selectedEntry.metadata.fields as field (`${field.kind}-${field.key}`)}
										{#if field.kind === 'stage'}
											<Field.FieldSet>
												<Field.FieldLegend>{field.label}</Field.FieldLegend>
												<Field.FieldDescription
													>{field.help}</Field.FieldDescription
												>
												<ToggleGroup.Root
													type="single"
													value={fieldValue(field)}
													variant="outline"
													class="w-full"
													onValueChange={updateStage}
													aria-label={field.label}
													data-testid="assistant-stage"
												>
													<ToggleGroup.Item
														value="title_abstract"
														class="grow"
													>
														Title/abstract
													</ToggleGroup.Item>
													<ToggleGroup.Item
														value="full_text"
														class="grow"
													>
														Full text
													</ToggleGroup.Item>
												</ToggleGroup.Root>
												{#if validationErrors[field.key]}
													<Field.FieldError
														>{validationErrors[
															field.key
														]}</Field.FieldError
													>
												{/if}
											</Field.FieldSet>
										{:else}
											<Field.Field
												data-invalid={Boolean(validationErrors[field.key])}
											>
												<Field.FieldLabel for={fieldId(field)}
													>{field.label}</Field.FieldLabel
												>
												{#if field.kind === 'uuid-list'}
													<Textarea
														id={fieldId(field)}
														class="min-h-28"
														value={fieldValue(field)}
														oninput={(event) =>
															updateField(field.key, event)}
														aria-invalid={Boolean(
															validationErrors[field.key]
														)}
														placeholder="One UUID per line"
														data-testid={`assistant-field-${field.key}`}
													/>
												{:else}
													<Input
														id={fieldId(field)}
														type={field.kind === 'integer'
															? 'number'
															: 'text'}
														value={fieldValue(field)}
														oninput={(event) =>
															updateField(field.key, event)}
														aria-invalid={Boolean(
															validationErrors[field.key]
														)}
														maxlength={field.kind === 'text'
															? field.maxLength
															: undefined}
														min={field.kind === 'integer'
															? field.min
															: undefined}
														max={field.kind === 'integer'
															? field.max
															: undefined}
														data-testid={`assistant-field-${field.key}`}
													/>
												{/if}
												<Field.FieldDescription
													>{field.help}</Field.FieldDescription
												>
												{#if validationErrors[field.key]}
													<Field.FieldError
														>{validationErrors[
															field.key
														]}</Field.FieldError
													>
												{/if}
											</Field.Field>
										{/if}
									{/each}
								</Field.FieldGroup>

								{#if selectedEntry.metadata.kind === AssistantToolKind.proposal}
									<Alert.Root data-testid="assistant-proposal-notice">
										<ShieldCheckIcon data-icon="inline-start" />
										<Alert.Title>Reviewer proposal</Alert.Title>
										<Alert.Description>
											This invocation creates a proposal for human review. It
											does not change scientific state until a reviewer
											accepts it.
										</Alert.Description>
									</Alert.Root>
								{/if}

								<Button
									type="submit"
									disabled={validation?.kind !== 'valid' ||
										executeMutation.isPending}
									data-testid="assistant-execute"
								>
									{#if executeMutation.isPending}
										<Spinner data-icon="inline-start" />
										Running…
									{:else}
										<SendIcon data-icon="inline-start" />
										Run once
									{/if}
								</Button>
							</form>

							{#if executeMutation.error}
								{@const error = executionError(executeMutation.error)}
								<Alert.Root
									variant="destructive"
									class="mt-5"
									data-testid="assistant-execution-error"
								>
									<AlertCircleIcon data-icon="inline-start" />
									<Alert.Title>{error.title}</Alert.Title>
									<Alert.Description>{error.message}</Alert.Description>
									{#if error.retry}
										<Alert.Action>
											<Button
												variant="outline"
												size="sm"
												onclick={retryExecution}
												data-testid="assistant-execution-retry"
											>
												<RefreshCwIcon data-icon="inline-start" />
												Retry
											</Button>
										</Alert.Action>
									{/if}
								</Alert.Root>
							{/if}

							{#if assistantResult}
								{#if assistantResult.kind === 'read'}
									<section
										class="mt-5 flex flex-col gap-2"
										data-testid="assistant-read-result"
									>
										<div class="flex items-center gap-2">
											<CheckCircle2Icon
												class="size-4 text-green-600"
												aria-hidden={true}
											/>
											<h2 class="text-sm font-medium">Read result</h2>
										</div>
										<pre
											class="max-h-[32rem] overflow-auto rounded-md border bg-muted/30 p-4 text-xs leading-relaxed">{readResult(
												assistantResult.data
											)}</pre>
									</section>
								{:else}
									<Alert.Root
										class="mt-5"
										data-testid="assistant-proposal-receipt"
									>
										<ShieldCheckIcon data-icon="inline-start" />
										<Alert.Title>Proposal created</Alert.Title>
										<Alert.Description>
											Proposal ID: <code>{assistantResult.proposal_id}</code>.
											Review it before any scientific state changes are made.
										</Alert.Description>
										{#if reviewHref}
											<Alert.Action>
												<a
													href={resolve(reviewHref as Pathname)}
													class="text-sm font-medium underline underline-offset-4"
													data-testid="assistant-review-link"
												>
													Open human review
												</a>
											</Alert.Action>
										{/if}
									</Alert.Root>
								{/if}
							{/if}
						</Card.Content>
					</Card.Root>
				{/if}
			</div>
		</section>
	{/if}
</main>
