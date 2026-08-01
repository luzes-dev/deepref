<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import * as Field from '$lib/components/ui/field';
	import * as InputGroup from '$lib/components/ui/input-group';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as NumberField from '$lib/components/ui/number-field';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { statusVariant } from '$lib/api/helpers';
	import { createCreateIngestion } from '$lib/api/generated/ingestions/ingestions';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PlayIcon from '@lucide/svelte/icons/play';
	import { useProjectWorkspaceContext } from './context.svelte.js';

	const workspace = useProjectWorkspaceContext();
	const createIngestion = createCreateIngestion();

	const sortedIngestions = $derived(
		workspace.ingestions.toSorted((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at))
	);
	const formError = $derived(createIngestion.error?.message ?? workspace.ingestionsError ?? '');

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

<div class="grid h-full min-h-0 gap-4 p-4 lg:grid-cols-[380px_1fr]">
	<section class="rounded-md border">
		<div class="border-b p-4">
			<h1 class="text-xl font-semibold">New ingestion</h1>
			<p class="text-sm text-muted-foreground">
				Seed recursive Crossref fetching for {workspace.project.name}.
			</p>
		</div>
		<div class="p-4">
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
					<NumberField.Root bind:value={workspace.ingestionMaxDepth} min={1} max={3}>
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
		<div class="border-t p-4">
			<Button
				onclick={submitIngestion}
				disabled={!workspace.ingestionDraft.dois.trim() || createIngestion.isPending}
			>
				<PlayIcon data-icon="inline-start" />Start ingestion
			</Button>
		</div>
	</section>

	<section class="min-h-0 overflow-hidden rounded-md border">
		<div class="flex items-center justify-between gap-3 border-b p-4">
			<div>
				<h2 class="font-medium">Run history</h2>
				<p class="text-sm text-muted-foreground">{sortedIngestions.length} project runs</p>
			</div>
			<Badge variant="secondary">{sortedIngestions.length}</Badge>
		</div>
		{#if workspace.ingestionsLoading}
			<div class="flex flex-col gap-2 p-4">
				{#each [0, 1, 2, 3, 4, 5] as index (index)}
					<Skeleton class="h-12" />
				{/each}
			</div>
		{:else if sortedIngestions.length === 0}
			<Empty.Root class="min-h-80">
				<Empty.Header>
					<Empty.Title>No ingestions</Empty.Title>
					<Empty.Description
						>Start an ingestion to create this project's first run.</Empty.Description
					>
				</Empty.Header>
			</Empty.Root>
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
							<Table.Row data-selected={workspace.selectedIngestion === ingestion.id}>
								<Table.Cell>
									<Badge variant={statusVariant(ingestion.status)}
										>{ingestion.status}</Badge
									>
								</Table.Cell>
								<Table.Cell>{ingestion.seed_count}</Table.Cell>
								<Table.Cell>{ingestion.fetched_count}</Table.Cell>
								<Table.Cell>{ingestion.failed_count}</Table.Cell>
								<Table.Cell
									>{new Date(ingestion.created_at).toLocaleString()}</Table.Cell
								>
								<Table.Cell class="text-right">
									<Button
										variant="outline"
										size="sm"
										onclick={() => workspace.openIngestion(ingestion.id)}
									>
										Open
									</Button>
								</Table.Cell>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</div>
		{/if}
	</section>
</div>
