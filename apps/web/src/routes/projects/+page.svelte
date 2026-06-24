<script lang="ts">
	import { resolve } from '$app/paths';
	import * as Card from '$lib/components/ui/card';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Field from '$lib/components/ui/field';
	import * as Table from '$lib/components/ui/table';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import {
		Empty,
		EmptyContent,
		EmptyDescription,
		EmptyHeader,
		EmptyTitle
	} from '$lib/components/ui/empty';
	import { Input } from '$lib/components/ui/input';
	import { Textarea } from '$lib/components/ui/textarea';
	import { createCreateProject, createListProjects } from '$lib/api/generated/projects/projects';
	import PlusIcon from '@lucide/svelte/icons/plus';

	const projectsQueryResult = createListProjects();
	const createProjectResult = createCreateProject();

	let open = $state(false);
	let name = $state('');
	let description = $state('');

	async function createProject() {
		try {
			await createProjectResult.mutateAsync({
				data: {
					name,
					description,
					default_max_depth: 2
				}
			});
			name = '';
			description = '';
			open = false;
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold">Projects</h1>
			<p class="text-sm text-muted-foreground">Research citation graphs grouped by topic.</p>
		</div>
		<Dialog.Root bind:open>
			<Dialog.Trigger>
				{#snippet child({ props })}
					<Button {...props}><PlusIcon data-icon="inline-start" />New project</Button>
				{/snippet}
			</Dialog.Trigger>
			<Dialog.Content>
				<Dialog.Header>
					<Dialog.Title>Create project</Dialog.Title>
					<Dialog.Description
						>Define the research workspace for a DOI ingestion.</Dialog.Description
					>
				</Dialog.Header>
				<Field.FieldGroup>
					<Field.Field>
						<Field.FieldLabel for="project-name">Name</Field.FieldLabel>
						<Input id="project-name" bind:value={name} />
					</Field.Field>
					<Field.Field>
						<Field.FieldLabel for="project-description">Description</Field.FieldLabel>
						<Textarea id="project-description" bind:value={description} />
					</Field.Field>
				</Field.FieldGroup>
				<Dialog.Footer>
					<Button variant="outline" onclick={() => (open = false)}>Cancel</Button>
					<Button
						onclick={createProject}
						disabled={!name.trim() || createProjectResult.isPending}>Create</Button
					>
				</Dialog.Footer>
				{#if createProjectResult.error}
					<p class="text-sm text-destructive">{createProjectResult.error.message}</p>
				{/if}
			</Dialog.Content>
		</Dialog.Root>
	</div>

	{#if projectsQueryResult.error}
		<Card.Root>
			<Card.Header><Card.Title>API unavailable</Card.Title></Card.Header>
			<Card.Content class="text-sm text-muted-foreground"
				>{projectsQueryResult.error.message}</Card.Content
			>
		</Card.Root>
	{:else if projectsQueryResult.isPending}
		<Card.Root><Card.Content class="p-6">Loading projects...</Card.Content></Card.Root>
	{:else if (projectsQueryResult.data?.data.length ?? 0) === 0}
		<Empty>
			<EmptyHeader>
				<EmptyTitle>No projects</EmptyTitle>
				<EmptyDescription>Create a project before starting an ingestion.</EmptyDescription>
			</EmptyHeader>
			<EmptyContent
				><Button onclick={() => (open = true)}>Create project</Button></EmptyContent
			>
		</Empty>
	{:else}
		<Card.Root>
			<Card.Header>
				<Card.Title>Active projects</Card.Title>
				<Card.Description
					>{projectsQueryResult.data?.data.length} project{projectsQueryResult.data?.data
						.length === 1
						? ''
						: 's'}</Card.Description
				>
			</Card.Header>
			<Card.Content>
				<Table.Root>
					<Table.Header>
						<Table.Row>
							<Table.Head>Name</Table.Head>
							<Table.Head>Depth</Table.Head>
							<Table.Head>Updated</Table.Head>
							<Table.Head></Table.Head>
						</Table.Row>
					</Table.Header>
					<Table.Body>
						{#each projectsQueryResult.data?.data ?? [] as project (project.id)}
							<Table.Row>
								<Table.Cell>
									<div class="font-medium">{project.name}</div>
									<div class="text-sm text-muted-foreground">
										{project.description}
									</div>
								</Table.Cell>
								<Table.Cell
									><Badge variant="secondary"
										>Depth {project.default_max_depth}</Badge
									></Table.Cell
								>
								<Table.Cell
									>{new Date(project.updated_at).toLocaleString()}</Table.Cell
								>
								<Table.Cell class="text-right"
									><Button
										variant="outline"
										size="sm"
										href={resolve(`/projects/${project.id}`)}>Open</Button
									></Table.Cell
								>
							</Table.Row>
						{/each}
					</Table.Body>
				</Table.Root>
			</Card.Content>
		</Card.Root>
	{/if}
</div>
