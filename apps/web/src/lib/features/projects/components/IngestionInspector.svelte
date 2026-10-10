<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Table from '@deepref/ui/table';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Progress } from '@deepref/ui/progress';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { shouldPollIngestion } from '#lib/api/helpers.js';
	import {
		createCancelIngestion,
		createGetIngestion,
		createListIngestionItems
	} from '#lib/api/generated/ingestions/ingestions.js';
	import { StatePanel } from '@deepref/ui/layout';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import {
		ingestionItemStatusLabel,
		ingestionItemStatusVariant,
		pluralize,
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

	const ingestionQuery = createGetIngestion(
		() => workspace.selectedIngestion ?? '',
		() => ({
			query: {
				enabled: Boolean(workspace.selectedIngestion),
				staleTime: 0,
				refetchInterval: (query) => shouldPollIngestion(query.state.data?.data.status),
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const ingestion = $derived(ingestionQuery.data?.data);
	const itemsQuery = createListIngestionItems(
		() => workspace.selectedIngestion ?? '',
		() => ({
			query: {
				enabled: Boolean(workspace.selectedIngestion),
				staleTime: 0,
				// Keep polling while an article is still in flight, even after the run status has
				// settled: the item list can lag the run status by one poll.
				refetchInterval: (query) =>
					shouldPollIngestion(ingestion?.status) !== false ||
					(query.state.data?.data.items ?? []).some(
						(item) => item.status === 'queued' || item.status === 'fetching'
					)
						? 2_000
						: false,
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const cancelIngestion = createCancelIngestion();
	const items = $derived(itemsQuery.data?.data.items ?? []);
	// The items endpoint only returns DOIs, so titles come from the workspace articles.
	const titleByDoi = $derived(
		new Map(
			workspace.articles.flatMap((article) =>
				article.doi && article.title ? [[article.doi.toLowerCase(), article.title]] : []
			)
		)
	);
	const polling = $derived(shouldPollIngestion(ingestion?.status) !== false);
	const display = $derived(
		ingestion ? runStatusDisplay(ingestion.status, ingestion.failed_count) : undefined
	);
	const isFetching = $derived(ingestionQuery.isFetching || itemsQuery.isFetching);
	const dataUpdatedAt = $derived(
		Math.max(ingestionQuery.dataUpdatedAt, itemsQuery.dataUpdatedAt)
	);
	const queryError = $derived(ingestionQuery.error ?? itemsQuery.error);
	const progress = $derived(
		ingestion
			? Math.round(
					((ingestion.fetched_count + ingestion.failed_count) /
						Math.max(
							ingestion.fetched_count +
								ingestion.failed_count +
								ingestion.queued_count,
							1
						)) *
						100
				)
			: 0
	);

	async function cancel() {
		if (!workspace.selectedIngestion) return;
		try {
			await cancelIngestion.mutateAsync({ ingestionId: workspace.selectedIngestion });
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<aside
	class="flex h-full min-h-0 flex-col border-l bg-background"
	data-testid="ingestion-inspector"
	data-selected={workspace.selectedIngestion ? 'true' : 'false'}
>
	{#if collapsed}
		<div class="flex h-full flex-col items-center gap-3 border-b px-2 py-4">
			<Button
				variant="ghost"
				size="icon"
				onclick={onToggleCollapse}
				aria-label="Expand ingestion inspector"
			>
				<PanelRightOpenIcon data-icon />
			</Button>
			<span class="text-2xs text-muted-foreground">Run</span>
		</div>
	{:else}
		<div class="flex items-center justify-between gap-2 border-b p-4">
			<div class="min-w-0">
				<h2 class="truncate text-sm font-medium">Import run</h2>
			</div>
			<div class="flex items-center gap-1">
				<Button
					variant="ghost"
					size="icon"
					class="hidden md:inline-flex"
					onclick={onToggleCollapse}
					aria-label="Collapse ingestion inspector"
				>
					<PanelRightCloseIcon data-icon />
				</Button>
				<Button
					variant="ghost"
					size="icon"
					class="hidden md:inline-flex"
					onclick={workspace.clearIngestion}
					aria-label="Clear ingestion"
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
			aria-label="Import details"
		>
			{#if !workspace.selectedIngestion}
				<StatePanel
					state="empty"
					title="No ingestion selected"
					description="Select a run to inspect its progress, polling state, and fetched items."
				/>
			{:else if queryError || cancelIngestion.error}
				<Alert.Root variant="destructive">
					<CircleAlertIcon />
					<Alert.Title>Ingestion unavailable</Alert.Title>
					<Alert.Description
						>{cancelIngestion.error?.message ?? queryError?.message}</Alert.Description
					>
					<Alert.Action
						onclick={() => {
							void ingestionQuery.refetch();
							void itemsQuery.refetch();
						}}>Try again</Alert.Action
					>
				</Alert.Root>
			{:else if ingestionQuery.isPending}
				<div class="flex flex-col gap-3">
					<Skeleton class="h-24" />
					<Skeleton class="h-64" />
				</div>
			{:else if ingestion && ingestion.project_id !== workspace.project.id}
				<Alert.Root data-testid="ingestion-degraded">
					<CircleAlertIcon />
					<Alert.Title>Ingestion belongs to another project</Alert.Title>
					<Alert.Description
						>This run belongs to project {ingestion.project_id}.</Alert.Description
					>
					<Alert.Action
						onclick={() => workspace.switchToIngestionProject(ingestion.project_id)}
						>Switch project</Alert.Action
					>
				</Alert.Root>
			{:else if ingestion}
				<div class="flex flex-col gap-4">
					<section aria-label="Run progress" class="flex flex-col gap-3">
						<div class="flex flex-wrap items-center justify-between gap-3">
							<h3 class="text-sm font-medium">
								Status {#if display}<Badge
										variant={display.variant}
										class="whitespace-nowrap"
										data-testid="run-status">{display.label}</Badge
									>{/if}
							</h3>
							<Button
								variant="outline"
								size="sm"
								onclick={cancel}
								disabled={cancelIngestion.isPending || !polling}>Cancel</Button
							>
						</div>
						<Progress value={progress} />
						{#if polling}
							<p class="text-xs text-muted-foreground tabular-nums">
								{ingestion.fetched_count} fetched · {ingestion.queued_count} queued{ingestion.failed_count >
								0
									? ` · ${ingestion.failed_count} not fetched`
									: ''}
							</p>
						{/if}
					</section>
					<section aria-label="Run articles" class="flex flex-col gap-2">
						<h3 class="text-sm font-semibold">
							Articles <span class="font-normal text-muted-foreground"
								>({items.length})</span
							>
						</h3>
						{#if !polling && ingestion.failed_count > 0}
							<p class="text-xs text-muted-foreground" data-testid="run-misses">
								{pluralize(ingestion.failed_count, 'article')} could not be fetched. Check
								the DOIs, then use Re-fetch metadata in the runs table to try them again.
							</p>
						{/if}
						<div class="max-h-96 overflow-auto">
							<Table.Root containerLabel="Ingestion articles">
								<Table.Header>
									<Table.Row>
										<Table.Head>Article and status</Table.Head>
									</Table.Row>
								</Table.Header>
								<Table.Body>
									{#each items as item (item.doi)}
										{@const title = titleByDoi.get(item.doi.toLowerCase())}
										<Table.Row>
											<Table.Cell class="max-w-full whitespace-normal">
												{#if title}
													<p class="line-clamp-2 text-sm">{title}</p>
												{/if}
												<Badge
													variant={ingestionItemStatusVariant(
														item.status
													)}
													class="my-1 whitespace-nowrap"
													>{ingestionItemStatusLabel(item.status)}</Badge
												>
												<p
													class="truncate text-xs text-muted-foreground"
													title={item.doi}
												>
													{item.doi}{item.depth > 0
														? ` · cited, depth ${item.depth}`
														: ''}
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
												No ingestion items yet.
											</Table.Cell>
										</Table.Row>
									{/each}
								</Table.Body>
							</Table.Root>
						</div>
					</section>
					<details class="text-xs text-muted-foreground">
						<summary class="cursor-pointer">Run & refresh details</summary>
						<div class="mt-3 flex flex-col gap-3">
							<p class="break-all">Run {ingestion.id}</p>
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
							<Button
								variant="outline"
								size="sm"
								onclick={() => {
									void ingestionQuery.refetch();
									void itemsQuery.refetch();
								}}
								disabled={isFetching}
								><RefreshCwIcon data-icon="inline-start" />Refresh now</Button
							>
						</div>
					</details>
				</div>
			{:else}
				<StatePanel
					state="error"
					title="Ingestion details unavailable"
					description="The selected run did not return a usable record. Refresh or clear the selection."
				/>
			{/if}
		</div>
	{/if}
</aside>
