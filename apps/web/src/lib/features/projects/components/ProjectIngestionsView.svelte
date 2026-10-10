<script lang="ts">
	import * as Table from '@deepref/ui/table';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as ToggleGroup from '@deepref/ui/toggle-group';
	import PaginationLoadMore from '@deepref/ui/pagination-load-more';
	import { StatePanel } from '@deepref/ui/layout';
	import { ApiError } from '#lib/api/custom-fetch.js';
	import { shouldPollIngestion } from '#lib/api/helpers.js';
	import {
		getListAcquisitionsQueryKey,
		listAcquisitions,
		refreshAcquisition
	} from '#lib/api/generated/acquisitions/acquisitions.js';
	import type { AcquisitionDto, IngestionDto } from '#lib/api/generated/models/index.js';
	import { getListIngestionsQueryKey } from '#lib/api/generated/ingestions/ingestions.js';
	import { createGetProjectPrisma } from '#lib/api/generated/review/review.js';
	import { resolve } from '$app/paths';
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { importRunFormatLabel, runStatusDisplay } from '../imports';
	import ImportDoiForm from './ImportDoiForm.svelte';
	import ImportFileForm from './ImportFileForm.svelte';
	import ImportPmidForm from './ImportPmidForm.svelte';

	type ImportSource = 'dois' | 'file' | 'pmids';

	const SOURCE_DESCRIPTION: Record<ImportSource, string> = {
		dois: "Paste DOIs. DeepRef fetches each article and follows its citations to build the review's corpus.",
		file: 'Upload a reference-manager export. Its records are added as they are, then checked for duplicates.',
		pmids: 'Paste PubMed IDs. DeepRef looks each one up in PubMed, adds the articles it finds, and checks them for duplicates.'
	};

	const workspace = useProjectWorkspaceContext();
	const queryClient = useQueryClient();
	const projectId = $derived(workspace.project.id);

	type RefreshState =
		| { kind: 'pending'; idempotencyKey: string }
		| { kind: 'failed'; idempotencyKey: string; message: string; retriable: boolean }
		| { kind: 'key-unavailable'; message: string };

	let refreshStates = $state<Record<string, RefreshState>>({});
	let source = $state<ImportSource>('dois');
	let doiNotice = $state<string | undefined>();

	const sortedIngestions = $derived(
		workspace.ingestions.toSorted((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at))
	);

	// File-import runs live in the acquisitions list, which DOI runs share; filter on the server.
	const fileRunsQuery = createInfiniteQuery(() => ({
		queryKey: getListAcquisitionsQueryKey(projectId, { strategy: 'file_import' }),
		queryFn: ({ pageParam, signal }) =>
			listAcquisitions(
				projectId,
				{ strategy: 'file_import', cursor: pageParam || undefined, limit: 50 },
				{ signal }
			),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		enabled: Boolean(projectId),
		staleTime: 0
	}));
	const fileRuns = $derived(fileRunsQuery.data?.pages.flatMap((page) => page.data.items) ?? []);

	// PubMed ID runs are acquisition runs too, so the server filters them the same way.
	const pmidRunsQuery = createInfiniteQuery(() => ({
		queryKey: getListAcquisitionsQueryKey(projectId, { strategy: 'pmid_import' }),
		queryFn: ({ pageParam, signal }) =>
			listAcquisitions(
				projectId,
				{ strategy: 'pmid_import', cursor: pageParam || undefined, limit: 50 },
				{ signal }
			),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		enabled: Boolean(projectId),
		// A PubMed run that is still queued or running is re-read, so its row settles on its own.
		refetchInterval: (query) =>
			(query.state.data?.pages ?? []).some((page) =>
				page.data.items.some((run) => shouldPollIngestion(run.status) !== false)
			)
				? 2_000
				: false,
		staleTime: 0
	}));
	const pmidRuns = $derived(pmidRunsQuery.data?.pages.flatMap((page) => page.data.items) ?? []);

	const prismaQuery = createGetProjectPrisma(
		() => projectId,
		() => ({
			query: { enabled: Boolean(projectId), staleTime: 0, refetchOnWindowFocus: 'always' }
		})
	);
	const unresolvedRecords = $derived(prismaQuery.data?.data.unresolved_records ?? 0);

	type RunRow = {
		key: string;
		createdAt: string;
		status: string;
		failedCount: number;
		fetchedCount: number | undefined;
		records: number;
		source: string;
		ingestion?: IngestionDto;
		acquisition?: AcquisitionDto;
	};

	const runRows = $derived.by<RunRow[]>(() => {
		const doiRuns: RunRow[] = sortedIngestions.map((ingestion) => ({
			key: `doi:${ingestion.id}`,
			createdAt: ingestion.created_at,
			status: ingestion.status,
			failedCount: ingestion.failed_count,
			fetchedCount: ingestion.fetched_count,
			records: ingestion.seed_count,
			source: `DOIs · depth ${ingestion.max_depth}`,
			ingestion
		}));
		const fileImports: RunRow[] = fileRuns.map((run: AcquisitionDto) => ({
			key: `file:${run.id}`,
			createdAt: run.created_at,
			status: run.status,
			failedCount: run.failed_count,
			fetchedCount: undefined,
			records: run.queued_count,
			source: importRunFormatLabel(run.format)
		}));
		const pmidImports: RunRow[] = pmidRuns.map((run: AcquisitionDto) => ({
			key: `pmid:${run.id}`,
			createdAt: run.created_at,
			status: run.status,
			failedCount: run.failed_count,
			fetchedCount: run.fetched_count,
			records: run.seed_count,
			source: importRunFormatLabel(run.format),
			acquisition: run
		}));
		return [...doiRuns, ...fileImports, ...pmidImports].sort(
			(a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt)
		);
	});

	const runsLoading = $derived(workspace.ingestionsLoading);
	const runsError = $derived(workspace.ingestionsError);
	const importRunsError = $derived(
		fileRunsQuery.isError || pmidRunsQuery.isError
			? 'Some imports could not be loaded.'
			: undefined
	);

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

	function loadDoiFile(text: string, fileName: string) {
		const existing = workspace.ingestionDraft.dois.trim();
		workspace.ingestionDraft.dois = [existing, text.trim()].filter(Boolean).join('\n');
		source = 'dois';
		doiNotice = `Loaded ${fileName} into the DOI list. Check it, then import.`;
	}
</script>

<PageTemplate testId="imports-page" maxWidth="full">
	<div class="flex flex-col gap-6">
		<header class="flex flex-col gap-1">
			<h2 class="editorial-title text-xl">Import articles</h2>
			<p class="text-sm text-muted-foreground">{SOURCE_DESCRIPTION[source]}</p>
		</header>

		{#if unresolvedRecords > 0}
			<div
				class="flex max-w-3xl flex-wrap items-center justify-between gap-3 rounded-lg border border-info-border bg-info-surface px-4 py-3 text-sm"
				role="status"
				data-testid="duplicate-check-hint"
			>
				<p class="text-info">
					<span class="font-semibold tabular-nums">{unresolvedRecords}</span>
					{unresolvedRecords === 1 ? 'record is' : 'records are'} waiting for the duplicate
					check
				</p>
				<Button
					variant="outline"
					size="sm"
					href={resolve('/projects/[projectId]/discovery/duplicates', { projectId })}
				>
					Open deduplication
				</Button>
			</div>
		{/if}

		<ToggleGroup.Root
			type="single"
			variant="outline"
			size="sm"
			value={source}
			aria-label="Import source"
			data-testid="import-source"
			onValueChange={(value) => {
				if (value === 'dois' || value === 'file' || value === 'pmids') source = value;
			}}
		>
			<ToggleGroup.Item value="dois">DOIs</ToggleGroup.Item>
			<ToggleGroup.Item value="file">File</ToggleGroup.Item>
			<ToggleGroup.Item value="pmids">PMIDs</ToggleGroup.Item>
		</ToggleGroup.Root>

		{#if doiNotice && source === 'dois'}
			<p class="max-w-3xl text-sm text-muted-foreground" role="status">{doiNotice}</p>
		{/if}

		{#if source === 'dois'}
			<ImportDoiForm />
		{:else if source === 'file'}
			<ImportFileForm onDoiList={loadDoiFile} />
		{:else}
			<ImportPmidForm />
		{/if}

		{#if runRows.length > 0 || runsLoading || runsError || importRunsError}
			<section aria-label="Run history" class="flex min-h-0 flex-1 flex-col gap-3">
				<h3 class="text-sm font-semibold" role="status">
					Runs <span class="font-normal text-muted-foreground tabular-nums"
						>{runRows.length}{runsLoading ? ' · refreshing' : ''}</span
					>
				</h3>
				{#if runsError}
					<div class="p-4 sm:p-5">
						<StatePanel
							state="error"
							title="Import history unavailable"
							description={runsError}
						/>
					</div>
				{:else if runsLoading}
					<div class="flex flex-col gap-2 p-4 sm:p-5" aria-label="Loading import history">
						{#each [0, 1, 2, 3, 4, 5] as index (index)}
							<Skeleton class="h-12" />
						{/each}
					</div>
				{:else}
					{#if importRunsError}
						<p class="px-4 pt-4 text-sm text-destructive sm:px-5" role="alert">
							{importRunsError}
						</p>
					{/if}
					<div class="max-h-full overflow-auto">
						<Table.Root containerLabel="Project ingestion runs">
							<Table.Header>
								<Table.Row>
									<Table.Head>Status</Table.Head>
									<Table.Head>Source</Table.Head>
									<Table.Head>Records</Table.Head>
									<Table.Head>Fetched</Table.Head>
									<Table.Head>Not fetched</Table.Head>
									<Table.Head>Created</Table.Head>
									<Table.Head class="text-right">Action</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each runRows as run (run.key)}
									{@const display = runStatusDisplay(run.status, run.failedCount)}
									{@const refreshState = run.ingestion
										? refreshStates[run.ingestion.id]
										: undefined}
									<Table.Row
										data-selected={(run.ingestion !== undefined &&
											workspace.selectedIngestion === run.ingestion.id) ||
											(run.acquisition !== undefined &&
												workspace.selectedAcquisition ===
													run.acquisition.id)}
										data-ingestion-id={run.ingestion?.id}
										data-run-kind={run.ingestion
											? 'doi'
											: run.acquisition
												? 'pmid'
												: 'file'}
										data-refresh-pending={refreshState?.kind === 'pending'}
									>
										<Table.Cell>
											<Badge
												variant={display.variant}
												class="whitespace-nowrap">{display.label}</Badge
											>
										</Table.Cell>
										<Table.Cell class="whitespace-nowrap"
											>{run.source}</Table.Cell
										>
										<Table.Cell class="tabular-nums">{run.records}</Table.Cell>
										<Table.Cell class="tabular-nums"
											>{run.fetchedCount ?? '—'}</Table.Cell
										>
										<Table.Cell class="tabular-nums"
											>{run.ingestion || run.acquisition
												? run.failedCount
												: '—'}</Table.Cell
										>
										<Table.Cell
											>{new Date(run.createdAt).toLocaleString()}</Table.Cell
										>
										<Table.Cell class="text-right">
											{#if run.acquisition}
												{@const acquisition = run.acquisition}
												<Button
													variant="outline"
													size="sm"
													onclick={() =>
														workspace.openAcquisition(acquisition.id)}
												>
													Open
												</Button>
											{:else if run.ingestion}
												{@const ingestion = run.ingestion}
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
															title="Fetch the latest metadata and citations again for this run"
															onclick={() =>
																refreshProvider(ingestion.id)}
															disabled={refreshState?.kind ===
																'pending'}
														>
															<RefreshCwIcon
																data-icon="inline-start"
															/>
															{refreshState?.kind === 'pending'
																? 'Re-fetching…'
																: refreshState?.kind === 'failed' &&
																	  refreshState.retriable
																	? 'Retry re-fetch'
																	: 'Re-fetch metadata'}
														</Button>
													{/if}
												</div>
												{#if refreshState?.kind === 'pending'}
													<p
														class="mt-2 text-xs text-muted-foreground"
														role="status"
													>
														Re-fetching metadata…
													</p>
												{:else if refreshState?.kind === 'failed' || refreshState?.kind === 'key-unavailable'}
													<div
														class="mt-2 flex flex-wrap items-center justify-end gap-2"
														role="alert"
													>
														<span
															class="max-w-64 text-xs text-destructive"
															>{refreshState.message}</span
														>
													</div>
												{/if}
											{/if}
										</Table.Cell>
									</Table.Row>
								{/each}
							</Table.Body>
						</Table.Root>
						{#if workspace.ingestionsHasNextPage || fileRunsQuery.hasNextPage || pmidRunsQuery.hasNextPage}
							<div class="border-t p-4">
								<PaginationLoadMore
									hasNextPage={workspace.ingestionsHasNextPage ||
										fileRunsQuery.hasNextPage ||
										pmidRunsQuery.hasNextPage}
									isLoading={workspace.ingestionsLoadingMore ||
										fileRunsQuery.isFetchingNextPage ||
										pmidRunsQuery.isFetchingNextPage}
									onLoadMore={() => {
										if (workspace.ingestionsHasNextPage)
											workspace.loadMoreIngestions();
										if (fileRunsQuery.hasNextPage)
											void fileRunsQuery.fetchNextPage();
										if (pmidRunsQuery.hasNextPage)
											void pmidRunsQuery.fetchNextPage();
									}}
								/>
							</div>
						{/if}
					</div>
				{/if}
			</section>
		{/if}
	</div>
</PageTemplate>
