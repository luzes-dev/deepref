<script lang="ts">
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Textarea } from '$lib/components/ui/textarea';
	import { createDeleteProject, createUpdateProject } from '$lib/api/generated/projects/projects';
	import type { ProjectDto } from '$lib/api/generated/models';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import TrashIcon from '@lucide/svelte/icons/trash-2';

	let {
		project
	}: {
		project: ProjectDto;
	} = $props();

	const workspace = useProjectWorkspaceContext();
	const updateProject = createUpdateProject();
	const deleteProject = createDeleteProject();

	let nameDraft = $state<string | undefined>(undefined);
	let descriptionDraft = $state<string | undefined>(undefined);
	let savedNameOverride = $state<string | undefined>(undefined);
	let savedDescriptionOverride = $state<string | undefined>(undefined);
	let deletePending = $state(false);

	const isBusy = $derived(updateProject.isPending || deleteProject.isPending);
	const savedName = $derived(savedNameOverride ?? project.name);
	const savedDescription = $derived(savedDescriptionOverride ?? project.description ?? '');
	const name = $derived(nameDraft ?? savedName);
	const description = $derived(descriptionDraft ?? savedDescription);
	const isDirty = $derived(name.trim() !== savedName || description.trim() !== savedDescription);
	const canSave = $derived(Boolean(name.trim()) && isDirty && !isBusy);

	async function saveProject() {
		if (!canSave) return;

		const trimmedName = name.trim();
		const trimmedDescription = description.trim();

		try {
			const result = await updateProject.mutateAsync({
				projectId: project.id,
				data: {
					name: trimmedName,
					description: trimmedDescription || null,
					default_max_depth: project.default_max_depth
				}
			});

			nameDraft = undefined;
			descriptionDraft = undefined;
			savedNameOverride = result.data.name;
			savedDescriptionOverride = result.data.description ?? '';
		} catch {
			// Mutation state renders the API error.
		}
	}

	function updateName(event: Event) {
		nameDraft = (event.currentTarget as HTMLInputElement).value;
	}

	function updateDescription(event: Event) {
		descriptionDraft = (event.currentTarget as HTMLTextAreaElement).value;
	}

	async function confirmDelete() {
		try {
			await deleteProject.mutateAsync({ projectId: project.id });
			workspace.finishProjectDeleted(project.id);
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<Field.FieldSet class="rounded-lg border p-4">
	<Field.FieldLegend class="flex min-w-0 items-center justify-between gap-3">
		<span class="truncate">{savedName}</span>
	</Field.FieldLegend>

	<Field.FieldGroup class="gap-4">
		<Field.Field>
			<Field.FieldLabel for={`management-project-name-${project.id}`}>Name</Field.FieldLabel>
			<Input
				id={`management-project-name-${project.id}`}
				value={name}
				oninput={updateName}
				disabled={isBusy}
				aria-invalid={!name.trim()}
			/>
		</Field.Field>
		<Field.Field>
			<Field.FieldLabel for={`management-project-description-${project.id}`}>
				Description
			</Field.FieldLabel>
			<Textarea
				id={`management-project-description-${project.id}`}
				value={description}
				oninput={updateDescription}
				disabled={isBusy}
			/>
		</Field.Field>
	</Field.FieldGroup>

	{#if updateProject.error}
		<p class="text-sm text-destructive">{updateProject.error.message}</p>
	{/if}
	{#if deleteProject.error}
		<p class="text-sm text-destructive">{deleteProject.error.message}</p>
	{/if}

	<div class="flex flex-col-reverse gap-2 sm:flex-row sm:items-center sm:justify-between">
		{#if deletePending}
			<div class="flex flex-col-reverse gap-2 sm:flex-row">
				<Button variant="outline" disabled={isBusy} onclick={() => (deletePending = false)}>
					Cancel
				</Button>
				<Button variant="destructive" disabled={isBusy} onclick={confirmDelete}>
					{#if deleteProject.isPending}
						<Spinner data-icon="inline-start" />
					{/if}
					Confirm delete
				</Button>
			</div>
		{:else}
			<Button variant="destructive" disabled={isBusy} onclick={() => (deletePending = true)}>
				<TrashIcon data-icon="inline-start" />
				Delete
			</Button>
		{/if}

		<Button disabled={!canSave} onclick={saveProject}>
			{#if updateProject.isPending}
				<Spinner data-icon="inline-start" />
			{/if}
			Save changes
		</Button>
	</div>
</Field.FieldSet>
