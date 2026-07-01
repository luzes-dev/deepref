<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Progress } from '$lib/components/ui/progress';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { statusVariant, shouldPollIngestion } from '$lib/api/helpers';
	import {
		createCancelIngestion,
		createGetIngestion,
		createListIngestionItems
	} from '$lib/api/generated/ingestions/ingestions';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from './context.svelte.js';

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
				refetchInterval: shouldPollIngestion(ingestion?.status),
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const cancelIngestion = createCancelIngestion();
	const items = $derived(itemsQuery.data?.data ?? []);
	const polling = $derived(shouldPollIngestion(ingestion?.status) !== false);
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

<aside class="flex h-full min-h-0 flex-col border-l bg-background">
	<div class="flex items-center justify-between gap-2 border-b p-4">
		<div class="min-w-0">
			<h2 class="truncate font-medium">Ingestion inspector</h2>
			<p class="truncate text-xs text-muted-foreground">
				{workspace.selectedIngestion ?? 'No ingestion selected'}
			</p>
		</div>
		<Button
			variant="ghost"
			size="icon"
			onclick={workspace.clearIngestion}
			aria-label="Clear ingestion"
		>
			<XIcon data-icon />
		</Button>
	</div>

	<div class="min-h-0 flex-1 overflow-auto p-4">
		{#if !workspace.selectedIngestion}
			<div
				class="flex h-full items-center justify-center text-center text-sm text-muted-foreground"
			>
				Select an ingestion run to inspect its progress and items.
			</div>
		{:else if queryError || cancelIngestion.error}
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>Ingestion unavailable</Alert.Title>
				<Alert.Description
					>{cancelIngestion.error?.message ?? queryError?.message}</Alert.Description
				>
			</Alert.Root>
		{:else if ingestionQuery.isPending}
			<div class="flex flex-col gap-3">
				<Skeleton class="h-24" />
				<Skeleton class="h-64" />
			</div>
		{:else if ingestion && ingestion.project_id !== workspace.project.id}
			<Alert.Root>
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
				<section class="rounded-md border p-4">
					<div class="flex flex-wrap items-center justify-between gap-3">
						<h3 class="font-medium">
							Status <Badge variant={statusVariant(ingestion.status)}
								>{ingestion.status}</Badge
							>
						</h3>
						<Button
							variant="outline"
							size="sm"
							onclick={cancel}
							disabled={cancelIngestion.isPending || !polling}
						>
							Cancel
						</Button>
					</div>
					<p class="mt-2 text-sm text-muted-foreground">
						{ingestion.fetched_count} fetched, {ingestion.failed_count} failed, {ingestion.queued_count}
						queued
					</p>
					<div class="mt-3"><Progress value={progress} /></div>
				</section>

				<section class="rounded-md border p-4">
					<div class="flex flex-col gap-3 text-sm">
						<div class="flex items-center justify-between gap-4">
							<span class="text-muted-foreground">Polling</span>
							<Badge variant={polling ? 'secondary' : 'outline'}
								>{polling ? 'Every 2 seconds' : 'Stopped'}</Badge
							>
						</div>
						<div class="flex items-center justify-between gap-4">
							<span class="text-muted-foreground">Request</span>
							<span>{isFetching ? 'Refreshing' : 'Idle'}</span>
						</div>
						<div class="flex items-center justify-between gap-4">
							<span class="text-muted-foreground">Last updated</span>
							<span
								>{dataUpdatedAt
									? new Date(dataUpdatedAt).toLocaleTimeString()
									: 'Never'}</span
							>
						</div>
						<Button
							variant="outline"
							onclick={() => {
								ingestionQuery.refetch();
								itemsQuery.refetch();
							}}
							disabled={isFetching}
						>
							<RefreshCwIcon data-icon="inline-start" />Refresh now
						</Button>
					</div>
				</section>

				<section class="overflow-hidden rounded-md border">
					<div class="border-b p-4">
						<h3 class="font-medium">Articles</h3>
						<p class="text-sm text-muted-foreground">
							{items.length} queued or fetched items
						</p>
					</div>
					<div class="max-h-96 overflow-auto">
						<Table.Root>
							<Table.Header>
								<Table.Row>
									<Table.Head>DOI</Table.Head>
									<Table.Head>Depth</Table.Head>
									<Table.Head>Status</Table.Head>
								</Table.Row>
							</Table.Header>
							<Table.Body>
								{#each items as item (item.doi)}
									<Table.Row>
										<Table.Cell class="max-w-48 truncate">{item.doi}</Table.Cell
										>
										<Table.Cell>{item.depth}</Table.Cell>
										<Table.Cell
											><Badge variant={statusVariant(item.status)}
												>{item.status}</Badge
											></Table.Cell
										>
									</Table.Row>
								{:else}
									<Table.Row>
										<Table.Cell
											colspan={3}
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
			</div>
		{/if}
	</div>
</aside>
