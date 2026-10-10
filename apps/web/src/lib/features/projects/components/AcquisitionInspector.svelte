<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Table from '@deepref/ui/table';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Progress } from '@deepref/ui/progress';
	import { Skeleton } from '@deepref/ui/skeleton';
	import PaginationLoadMore from '@deepref/ui/pagination-load-more';
	import { StatePanel } from '@deepref/ui/layout';
	import { createInfiniteQuery } from '@tanstack/svelte-query';
	import { shouldPollIngestion } from '#lib/api/helpers.js';
	import {
		createGetAcquisition,
		getListAcquisitionItemsQueryKey,
		listAcquisitionItems
	} from '#lib/api/generated/acquisitions/acquisitions.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import {
		pluralize,
		pmidItemStatusLabel,
		pmidItemStatusVariant,
		runStatusDisplay
	} from '../imports';

	let {
		collapsed = false,
		onToggleCollapse = () => {}
	}: {
		collapsed?: boolean;
		onToggleCollapse?: () => void;
	} = $props();

	const workspace = useProjectWorkspaceContext();
	const projectId = $derived(workspace.project.id);
	const acquisitionId = $derived(workspace.selectedAcquisition ?? '');

	const runQuery = createGetAcquisition(
		() => projectId,
		() => acquisitionId,
		() => ({
			query: {
				enabled: Boolean(acquisitionId),
				staleTime: 0,
				refetchInterval: (query) => shouldPollIngestion(query.state.data?.data.status),
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const run = $derived(runQuery.data?.data);
	const polling = $derived(shouldPollIngestion(run?.status) !== false);

	const itemsQuery = createInfiniteQuery(() => ({
		queryKey: getListAcquisitionItemsQueryKey(projectId, acquisitionId),
		queryFn: ({ pageParam, signal }) =>
			listAcquisitionItems(
				projectId,
				acquisitionId,
				{ cursor: pageParam || undefined, limit: 100 },
				{ signal }
			),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		enabled: Boolean(acquisitionId),
		staleTime: 0,
		// Keep polling while an ID is still waiting, even after the run has settled, so the
		// last answers are not missed.
		refetchInterval: (query) =>
			polling ||
			(query.state.data?.pages ?? []).some((page) =>
				page.data.items.some((item) => item.status === 'queued')
			)
				? 2_000
				: false,
		refetchIntervalInBackground: false,
		refetchOnWindowFocus: 'always'
	}));
	const items = $derived(itemsQuery.data?.pages.flatMap((page) => page.data.items) ?? []);

	const display = $derived(run ? runStatusDisplay(run.status, run.failed_count) : undefined);
	const progress = $derived(
		run && run.seed_count > 0
			? Math.round(((run.fetched_count + run.failed_count) / run.seed_count) * 100)
			: 0
	);
	const queryError = $derived(runQuery.error ?? itemsQuery.error);
	const isFetching = $derived(runQuery.isFetching || itemsQuery.isFetching);
	const dataUpdatedAt = $derived(Math.max(runQuery.dataUpdatedAt, itemsQuery.dataUpdatedAt));
</script>

<aside
	class="flex h-full min-h-0 flex-col border-l bg-background"
	data-testid="acquisition-inspector"
	data-selected={workspace.selectedAcquisition ? 'true' : 'false'}
>
	{#if collapsed}
		<div class="flex h-full flex-col items-center gap-3 border-b px-2 py-4">
			<Button
				variant="ghost"
				size="icon"
				onclick={onToggleCollapse}
				aria-label="Expand import run inspector"
			>
				<PanelRightOpenIcon data-icon />
			</Button>
			<span class="text-2xs text-muted-foreground">Run</span>
		</div>
	{:else}
		<div class="flex items-center justify-between gap-2 border-b p-4">
			<h2 class="truncate text-sm font-medium">PubMed import run</h2>
			<div class="flex items-center gap-1">
				<Button
					variant="ghost"
					size="icon"
					class="hidden md:inline-flex"
					onclick={onToggleCollapse}
					aria-label="Collapse import run inspector"
				>
					<PanelRightCloseIcon data-icon />
				</Button>
				<Button
					variant="ghost"
					size="icon"
					class="hidden md:inline-flex"
					onclick={workspace.clearIngestion}
					aria-label="Clear import run"
				>
					<XIcon data-icon />
				</Button>
			</div>
		</div>

		<!-- Keyboard users need to focus this region to scroll long details. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="min-h-0 flex-1 overflow-auto p-4"
			tabindex="0"
			role="region"
			aria-label="Import run details"
		>
			{#if queryError}
				<Alert.Root variant="destructive">
					<CircleAlertIcon />
					<Alert.Title>Import run unavailable</Alert.Title>
					<Alert.Description>{queryError.message}</Alert.Description>
					<Alert.Action
						onclick={() => {
							void runQuery.refetch();
							void itemsQuery.refetch();
						}}>Try again</Alert.Action
					>
				</Alert.Root>
			{:else if runQuery.isPending}
				<div class="flex flex-col gap-3">
					<Skeleton class="h-24" />
					<Skeleton class="h-64" />
				</div>
			{:else if run}
				<div class="flex flex-col gap-4">
					<section aria-label="Run progress" class="flex flex-col gap-3">
						{#if display}
							<h3 class="text-sm font-medium">
								Status <Badge
									variant={display.variant}
									class="whitespace-nowrap"
									data-testid="run-status">{display.label}</Badge
								>
							</h3>
						{/if}
						<Progress value={progress} />
						<p class="text-xs text-muted-foreground tabular-nums">
							{run.fetched_count} found in PubMed · {run.queued_count} waiting{run.failed_count >
							0
								? ` · ${run.failed_count} not found or not fetched`
								: ''}
						</p>
						{#if !polling && run.failed_count > 0}
							<p class="text-xs text-muted-foreground" data-testid="run-misses">
								{pluralize(run.failed_count, 'PubMed ID')} could not be fetched. Each
								one below says why.
							</p>
						{/if}
					</section>

					<section aria-label="Run PubMed IDs" class="flex flex-col gap-2">
						<h3 class="text-sm font-semibold">
							PubMed IDs <span class="font-normal text-muted-foreground"
								>({items.length}{itemsQuery.hasNextPage ? '+' : ''})</span
							>
						</h3>
						<div class="max-h-96 overflow-auto">
							<Table.Root containerLabel="PubMed IDs of this import">
								<Table.Header>
									<Table.Row>
										<Table.Head>Article and status</Table.Head>
									</Table.Row>
								</Table.Header>
								<Table.Body>
									{#each items as item (item.identifier)}
										<Table.Row data-status={item.status}>
											<Table.Cell class="max-w-full whitespace-normal">
												{#if item.title}
													<p class="line-clamp-2 text-sm">{item.title}</p>
												{/if}
												<Badge
													variant={pmidItemStatusVariant(item.status)}
													class="my-1 whitespace-nowrap"
													>{pmidItemStatusLabel(item.status)}</Badge
												>
												<p class="truncate text-xs text-muted-foreground">
													PMID {item.identifier}
												</p>
												{#if item.status === 'failed' && item.last_error}
													<p
														class="line-clamp-2 text-xs text-muted-foreground"
														title={item.last_error}
													>
														Reason: {item.last_error}
													</p>
												{/if}
											</Table.Cell>
										</Table.Row>
									{:else}
										<Table.Row>
											<Table.Cell
												class="h-24 text-center text-muted-foreground"
											>
												No PubMed IDs yet.
											</Table.Cell>
										</Table.Row>
									{/each}
								</Table.Body>
							</Table.Root>
							{#if itemsQuery.hasNextPage}
								<div class="border-t p-4">
									<PaginationLoadMore
										hasNextPage={itemsQuery.hasNextPage}
										isLoading={itemsQuery.isFetchingNextPage}
										onLoadMore={() => void itemsQuery.fetchNextPage()}
									/>
								</div>
							{/if}
						</div>
					</section>

					<details class="text-xs text-muted-foreground">
						<summary class="cursor-pointer">Run details</summary>
						<div class="mt-3 flex flex-col gap-2">
							<p class="break-all">Run {run.id}</p>
							<p>
								{polling ? 'Refreshes every 2 seconds' : 'Polling stopped'} · {isFetching
									? 'Refreshing'
									: 'Idle'}
							</p>
							<p>
								Last updated {dataUpdatedAt
									? new Date(dataUpdatedAt).toLocaleTimeString()
									: 'Never'}
							</p>
						</div>
					</details>
				</div>
			{:else}
				<StatePanel
					state="error"
					title="Import run unavailable"
					description="The selected run did not return a usable record. Refresh or clear the selection."
				/>
			{/if}
		</div>
	{/if}
</aside>
