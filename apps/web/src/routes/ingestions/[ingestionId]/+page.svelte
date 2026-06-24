<script lang="ts">
	import { page } from '$app/state';
	import * as Card from '$lib/components/ui/card';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Progress } from '$lib/components/ui/progress';
	import { statusVariant, shouldPollIngestion } from '$lib/api/helpers';
	import {
		createCancelIngestion,
		createGetIngestion,
		createListIngestionItems
	} from '$lib/api/generated/ingestions/ingestions';

	const ingestionId = $derived(page.params.ingestionId ?? '');
	const ingestionQuery = createGetIngestion(
		() => ingestionId,
		() => ({
			query: {
				enabled: Boolean(ingestionId),
				staleTime: 0,
				refetchInterval: (query) => shouldPollIngestion(query.state.data?.data.status),
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always'
			}
		})
	);
	const ingestion = $derived(ingestionQuery.data?.data);
	const itemsQuery = createListIngestionItems(
		() => ingestionId,
		() => ({
			query: {
				enabled: Boolean(ingestionId),
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
		try {
			await cancelIngestion.mutateAsync({ ingestionId });
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold">Ingestion</h1>
			<p class="text-sm text-muted-foreground">{ingestionId}</p>
		</div>
		<Button variant="outline" onclick={cancel} disabled={cancelIngestion.isPending || !polling}
			>Cancel</Button
		>
	</div>
	{#if ingestion}
		<Card.Root>
			<Card.Header>
				<Card.Title
					>Status <Badge variant={statusVariant(ingestion.status)}
						>{ingestion.status}</Badge
					></Card.Title
				>
				<Card.Description
					>{ingestion.fetched_count} fetched, {ingestion.failed_count} failed, {ingestion.queued_count}
					queued</Card.Description
				>
			</Card.Header>
			<Card.Content><Progress value={progress} /></Card.Content>
		</Card.Root>
	{:else if ingestionQuery.isPending}
		<Card.Root><Card.Content class="p-6">Loading ingestion...</Card.Content></Card.Root>
	{/if}
	<div class="grid gap-4 lg:grid-cols-[1fr_360px]">
		<Card.Root>
			<Card.Header><Card.Title>Articles</Card.Title></Card.Header>
			<Card.Content class="p-0">
				<Table.Root>
					<Table.Header
						><Table.Row
							><Table.Head>DOI</Table.Head><Table.Head>Depth</Table.Head><Table.Head
								>Status</Table.Head
							><Table.Head>Attempts</Table.Head></Table.Row
						></Table.Header
					>
					<Table.Body>
						{#each items as item (item.doi)}
							<Table.Row>
								<Table.Cell>{item.doi}</Table.Cell>
								<Table.Cell>{item.depth}</Table.Cell>
								<Table.Cell
									><Badge variant={statusVariant(item.status)}
										>{item.status}</Badge
									></Table.Cell
								>
								<Table.Cell>{item.attempts}</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
		<Card.Root class="min-h-[320px]">
			<Card.Header>
				<Card.Title>Live updates</Card.Title>
				<Card.Description>TanStack Query polling status</Card.Description>
			</Card.Header>
			<Card.Content class="flex flex-col gap-4 text-sm">
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
					<span>
						{dataUpdatedAt ? new Date(dataUpdatedAt).toLocaleTimeString() : 'Never'}
					</span>
				</div>
				{#if queryError || cancelIngestion.error}
					<p class="text-destructive">
						{cancelIngestion.error?.message ?? queryError?.message}
					</p>
				{/if}
				<Button
					variant="outline"
					onclick={() => {
						ingestionQuery.refetch();
						itemsQuery.refetch();
					}}
					disabled={isFetching}>Refresh now</Button
				>
			</Card.Content>
		</Card.Root>
	</div>
</div>
