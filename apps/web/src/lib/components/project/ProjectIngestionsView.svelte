<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Field from '$lib/components/ui/field';
	import * as InputGroup from '$lib/components/ui/input-group';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as NumberField from '$lib/components/ui/number-field';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import PaginationLoadMore from '$lib/components/PaginationLoadMore.svelte';
	import {
		MetricTile,
		PageHeader,
		PageToolbar,
		StatePanel,
		Surface
	} from '$lib/components/layout';
	import { statusVariant } from '$lib/api/helpers';
	import { ApiError } from '$lib/api/custom-fetch';
	import {
		createCreateIngestion,
		getListIngestionsQueryKey
	} from '$lib/api/generated/ingestions/ingestions';
	import { refreshAcquisition } from '$lib/api/generated/acquisitions/acquisitions';
	import { useQueryClient } from '@tanstack/svelte-query';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PlayIcon from '@lucide/svelte/icons/play';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	const workspace = useProjectWorkspaceContext();
	const createIngestion = createCreateIngestion();
	const queryClient = useQueryClient();

	type RefreshState =
		| { kind: 'pending'; idempotencyKey: string }
		| { kind: 'failed'; idempotencyKey: string; message: string; retriable: boolean }
		| { kind: 'key-unavailable'; message: string };

	let refreshStates = $state<Record<string, RefreshState>>({});

	const sortedIngestions = $derived(
		workspace.ingestions.toSorted((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at))
	);
	const formError = $derived(createIngestion.error?.message ?? workspace.ingestionsError ?? '');

	function refreshErrorMessage(error: unknown): string {
		const message = error instanceof Error ? error.message.trim() : '';
		const boundedMessage = message || 'The provider refresh could not be started.';
		return boundedMessage.length <= 240 ? boundedMessage : `${boundedMessage.slice(0, 237)}…`;
	}

	function isRetriableRefreshError(error: unknown): boolean {
		if (!(error instanceof ApiError)) return true;
		return (
			error.status === 408 ||
			error.status === 425 ||
			error.status === 429 ||
			error.status >= 500
		);
	}

	async function refreshProvider(ingestionId: string): Promise<void> {
		const previous = refreshStates[ingestionId];
		if (previous?.kind === 'pending') return;

		const idempotencyKey =
			previous?.kind === 'failed' && previous.retriable
				? previous.idempotencyKey
				: globalThis.crypto?.randomUUID?.();
		if (!idempotencyKey?.trim()) {
			refreshStates[ingestionId] = {
				kind: 'key-unavailable',
				message:
					'A secure refresh request key is unavailable. Try again in a supported browser.'
			};
			return;
		}

		refreshStates[ingestionId] = { kind: 'pending', idempotencyKey };
		try {
			const response = await refreshAcquisition(workspace.project.id, ingestionId, {
				headers: { 'Idempotency-Key': idempotencyKey }
			});
			try {
				await queryClient.invalidateQueries({ queryKey: getListIngestionsQueryKey() });
			} catch {
				// The workspace query renders any refetch error; the refresh itself succeeded.
			}
			delete refreshStates[ingestionId];
			workspace.openIngestion(response.data.id);
		} catch (error: unknown) {
			refreshStates[ingestionId] = {
				kind: 'failed',
				idempotencyKey,
				message: refreshErrorMessage(error),
				retriable: isRetriableRefreshError(error)
			};
		}
	}

	async function submitIngestion() {
		const seed_dois = workspace.ingestionDraft.dois
			.split(/[\n,]+/)
			.map((doi) => doi.trim())
			.filter(Boolean);
		try {
			const result = await createIngestion.mutateAsync({
				data: {
					project_id: workspace.project.id,
					seed_dois,
					max_depth: workspace.ingestionMaxDepth,
					metadata_provider: 'crossref',
					citation_provider: 'crossref'
				}
			});
			workspace.ingestionDraft.dois = '';
			workspace.openIngestion(result.data.id);
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<div class="flex h-full min-h-0 flex-col overflow-auto bg-background" data-testid="imports-page">
	<div class="mx-auto flex w-full max-w-[1440px] flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Evidence workspace / Collect"
			title="Imports"
			description="Seed recursive provider fetching and monitor every ingestion run."
		>
			<p class="mt-2 text-xs text-muted-foreground">Project: {workspace.project.name}</p>
		</PageHeader>

		<PageToolbar label="Import workflow status">
			<div class="flex flex-wrap items-center gap-2">
				<Badge variant="secondary">{sortedIngestions.length} runs</Badge>
				<Badge variant={workspace.ingestionsLoading ? 'outline' : 'default'}>
					{workspace.ingestionsLoading ? 'Refreshing' : 'Live history'}
				</Badge>
			</div>
		</PageToolbar>

		<div class="grid gap-3 sm:grid-cols-3" data-testid="ingestion-metrics">
			<MetricTile
				label="Runs"
				value={sortedIngestions.length.toLocaleString()}
				detail="project history"
				tone="info"
				class="[font-variant-numeric:tabular-nums]"
			/>
			<MetricTile
				label="Active"
				value={sortedIngestions
					.filter(
						(ingestion) =>
							ingestion.status === 'running' || ingestion.status === 'queued'
					)
					.length.toLocaleString()}
				detail="polling runs"
				tone="positive"
				class="[font-variant-numeric:tabular-nums]"
			/>
			<MetricTile
				label="Selected"
				value={workspace.selectedIngestion ? '1' : '0'}
				detail="run inspector"
				tone="default"
				class="[font-variant-numeric:tabular-nums]"
			/>
		</div>

		<div class="grid min-h-0 gap-5 lg:grid-cols-[minmax(18rem,24rem)_minmax(0,1fr)]">
			<Surface as="section" tone="default" class="flex min-h-0 flex-col overflow-hidden">
				<div class="border-b border-border/70 p-4 sm:p-5">
					<h2 class="text-lg font-semibold">Start an ingestion</h2>
					<p class="mt-1 text-sm text-muted-foreground">
						Seed recursive Crossref fetching for {workspace.project.name}.
					</p>
				</div>
				<div class="p-4 sm:p-5">
					<Field.FieldGroup>
						<Field.Field>
							<Field.FieldLabel>Project</Field.FieldLabel>
							<div class="rounded-md border bg-muted px-3 py-2 text-sm">
								{workspace.project.name}
							</div>
						</Field.Field>
						<Field.Field>
							<Field.FieldLabel for="dois">Seed DOIs</Field.FieldLabel>
							<InputGroup.Root>
								<InputGroup.Textarea
									id="dois"
									bind:value={workspace.ingestionDraft.dois}
									placeholder="10.1145/3366423.3380295"
									class="min-h-32"
								/>
							</InputGroup.Root>
						</Field.Field>
						<Field.Field>
							<Field.FieldLabel>Depth</Field.FieldLabel>
							<NumberField.Root
								bind:value={workspace.ingestionMaxDepth}
								min={1}
								max={3}
							>
								<NumberField.Group>
									<NumberField.Decrement />
									<NumberField.Input />
									<NumberField.Increment />
								</NumberField.Group>
							</NumberField.Root>
						</Field.Field>
					</Field.FieldGroup>
					{#if formError}
						<Alert.Root variant="destructive" class="mt-4">
							<CircleAlertIcon />
							<Alert.Title>Ingestion unavailable</Alert.Title>
							<Alert.Description>{formError}</Alert.Description>
						</Alert.Root>
					{/if}
				</div>
				<div class="border-t border-border/70 p-4 sm:p-5">
					<Button
						onclick={submitIngestion}
						disabled={!workspace.ingestionDraft.dois.trim() ||
							createIngestion.isPending}
					>
						<PlayIcon data-icon="inline-start" />Start ingestion
					</Button>
				</div>
			</Surface>

			<Surface as="section" tone="default" class="min-h-0 overflow-hidden">
				<div
					class="flex flex-wrap items-center justify-between gap-3 border-b border-border/70 p-4 sm:p-5"
				>
					<div>
						<h2 class="font-medium">Run history</h2>
						<p class="text-sm text-muted-foreground">
							{sortedIngestions.length} project runs
						</p>
					</div>
					<Badge variant="secondary">{sortedIngestions.length}</Badge>
				</div>
				{#if workspace.ingestionsError}
					<div class="p-4 sm:p-5">
						<StatePanel
							state="error"
							title="Ingestion history unavailable"
							description={workspace.ingestionsError}
						/>
					</div>
				{:else if workspace.ingestionsLoading}
					<div
						class="flex flex-col gap-2 p-4 sm:p-5"
						aria-label="Loading ingestion history"
					>
						{#each [0, 1, 2, 3, 4, 5] as index (index)}
							<Skeleton class="h-12" />
						{/each}
					</div>
				{:else if sortedIngestions.length === 0}
					<div class="p-4 sm:p-5">
						<StatePanel
							state="empty"
							title="No ingestions"
							description="Start an ingestion to create this project's first run."
						/>
					</div>
				{:else}
					<div class="max-h-full overflow-auto">
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head>Status</Table.Head>
									<Table.Head>Seeds</Table.Head>
									<Table.Head>Fetched</Table.Head>
									<Table.Head>Failed</Table.Head>
									<Table.Head>Created</Table.Head>
									<Table.Head></Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each sortedIngestions as ingestion (ingestion.id)}
									{@const refreshState = refreshStates[ingestion.id]}
									<Table.Row
										data-selected={workspace.selectedIngestion === ingestion.id}
										data-ingestion-id={ingestion.id}
										data-refresh-pending={refreshState?.kind === 'pending'}
									>
										<Table.Cell>
											<Badge variant={statusVariant(ingestion.status)}
												>{ingestion.status}</Badge
											>
										</Table.Cell>
										<Table.Cell>{ingestion.seed_count}</Table.Cell>
										<Table.Cell>{ingestion.fetched_count}</Table.Cell>
										<Table.Cell>{ingestion.failed_count}</Table.Cell>
										<Table.Cell
											>{new Date(
												ingestion.created_at
											).toLocaleString()}</Table.Cell
										>
										<Table.Cell class="text-right">
											<div class="flex flex-wrap justify-end gap-2">
												<Button
													variant="outline"
													size="sm"
													onclick={() =>
														workspace.openIngestion(ingestion.id)}
												>
													Open
												</Button>
												{#if ingestion.status === 'completed'}
													<Button
														variant="outline"
														size="sm"
														onclick={() =>
															refreshProvider(ingestion.id)}
														disabled={refreshState?.kind === 'pending'}
													>
														<RefreshCwIcon data-icon="inline-start" />
														{refreshState?.kind === 'pending'
															? 'Refreshing provider…'
															: refreshState?.kind === 'failed' &&
																  refreshState.retriable
																? 'Retry refresh provider'
																: 'Refresh provider'}
													</Button>
												{/if}
											</div>
											{#if refreshState?.kind === 'pending'}
												<p
													class="mt-2 text-xs text-muted-foreground"
													role="status"
												>
													Provider refresh is in progress.
												</p>
											{:else if refreshState?.kind === 'failed' || refreshState?.kind === 'key-unavailable'}
												<div
													class="mt-2 flex flex-wrap items-center justify-end gap-2"
													role="alert"
												>
													<span class="max-w-64 text-xs text-destructive"
														>{refreshState.message}</span
													>
												</div>
											{/if}
										</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
						<div class="border-t p-4">
							<PaginationLoadMore
								hasNextPage={workspace.ingestionsHasNextPage}
								isLoading={workspace.ingestionsLoadingMore}
								loadedCount={workspace.ingestions.length}
								label="project runs"
								onLoadMore={workspace.loadMoreIngestions}
							/>
						</div>
					</div>
				{/if}
			</Surface>
		</div>
	</div>
</div>
