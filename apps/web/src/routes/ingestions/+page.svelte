<script lang="ts">
	import { resolve } from '$app/paths';
	import * as Card from '$lib/components/ui/card';
	import * as Field from '$lib/components/ui/field';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Textarea } from '$lib/components/ui/textarea';
	import * as NumberField from '$lib/components/ui/number-field';
	import { statusVariant } from '$lib/api/helpers';
	import {
		createCreateIngestion,
		createListIngestions
	} from '$lib/api/generated/ingestions/ingestions';
	import { createListProjects } from '$lib/api/generated/projects/projects';

	const ingestionsQueryResult = createListIngestions();
	const projectsQueryResult = createListProjects();
	const createIngestionResult = createCreateIngestion();

	let selectedProjectId = $state('');
	let dois = $state('');
	let maxDepth = $state(2);
	const projectId = $derived(selectedProjectId || projectsQueryResult.data?.data[0]?.id || '');
	const error = $derived(
		createIngestionResult.error?.message ??
			ingestionsQueryResult.error?.message ??
			projectsQueryResult.error?.message ??
			''
	);

	async function createIngestion() {
		const seed_dois = dois
			.split(/[\n,]+/)
			.map((doi) => doi.trim())
			.filter(Boolean);
		try {
			await createIngestionResult.mutateAsync({
				data: {
					project_id: projectId,
					seed_dois,
					max_depth: maxDepth,
					metadata_provider: 'crossref',
					citation_provider: 'crossref'
				}
			});
			dois = '';
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<div class="grid gap-6 lg:grid-cols-[380px_1fr]">
	<Card.Root>
		<Card.Header>
			<Card.Title>New ingestion</Card.Title>
			<Card.Description
				>Seed recursive Crossref fetching from one or more DOIs.</Card.Description
			>
		</Card.Header>
		<Card.Content>
			<Field.FieldGroup>
				<Field.Field>
					<Field.FieldLabel for="project">Project</Field.FieldLabel>
					<select
						id="project"
						class="h-9 rounded-md border bg-background px-3 text-sm"
						value={projectId}
						onchange={(event) => (selectedProjectId = event.currentTarget.value)}
					>
						{#each projectsQueryResult.data?.data ?? [] as project (project.id)}
							<option value={project.id}>{project.name}</option>
						{/each}
					</select>
				</Field.Field>
				<Field.Field>
					<Field.FieldLabel for="dois">Seed DOIs</Field.FieldLabel>
					<Textarea id="dois" bind:value={dois} placeholder="10.1145/3366423.3380295" />
				</Field.Field>
				<Field.Field>
					<Field.FieldLabel>Depth</Field.FieldLabel>
					<NumberField.Root bind:value={maxDepth} min={1} max={3}>
						<NumberField.Group>
							<NumberField.Decrement />
							<NumberField.Input />
							<NumberField.Increment />
						</NumberField.Group>
					</NumberField.Root>
				</Field.Field>
			</Field.FieldGroup>
		</Card.Content>
		<Card.Footer
			><Button
				onclick={createIngestion}
				disabled={!projectId || !dois.trim() || createIngestionResult.isPending}
				>Start ingestion</Button
			></Card.Footer
		>
	</Card.Root>
	<Card.Root>
		<Card.Header>
			<Card.Title>Ingestions</Card.Title>
			<Card.Description
				>{error || `${ingestionsQueryResult.data?.data.length ?? 0} runs`}</Card.Description
			>
		</Card.Header>
		<Card.Content class="p-0">
			<Table.Root>
				<Table.Header>
					<Table.Row>
						<Table.Head>Status</Table.Head>
						<Table.Head>Seeds</Table.Head>
						<Table.Head>Fetched</Table.Head>
						<Table.Head>Failed</Table.Head>
						<Table.Head></Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each ingestionsQueryResult.data?.data ?? [] as ingestion (ingestion.id)}
						<Table.Row>
							<Table.Cell
								><Badge variant={statusVariant(ingestion.status)}
									>{ingestion.status}</Badge
								></Table.Cell
							>
							<Table.Cell>{ingestion.seed_count}</Table.Cell>
							<Table.Cell>{ingestion.fetched_count}</Table.Cell>
							<Table.Cell>{ingestion.failed_count}</Table.Cell>
							<Table.Cell class="text-right"
								><Button
									variant="outline"
									size="sm"
									href={resolve(`/ingestions/${ingestion.id}`)}>Open</Button
								></Table.Cell
							>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		</Card.Content>
	</Card.Root>
</div>
