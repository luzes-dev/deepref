<script lang="ts">
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Textarea } from '$lib/components/ui/textarea';
	import { createCreateProject } from '$lib/api/generated/projects/projects';
	import { DEFAULT_PROJECT_MAX_DEPTH } from './constants';

	let {
		onCreated,
		onCancel,
		nameInputId = 'project-name',
		descriptionInputId = 'project-description'
	}: {
		onCreated: (projectId: string) => void;
		onCancel: () => void;
		nameInputId?: string;
		descriptionInputId?: string;
	} = $props();

	const createProject = createCreateProject();

	let name = $state('');
	let description = $state('');

	async function submitProject() {
		try {
			const result = await createProject.mutateAsync({
				data: {
					name: name.trim(),
					description: description.trim(),
					default_max_depth: DEFAULT_PROJECT_MAX_DEPTH
				}
			});
			name = '';
			description = '';
			onCreated(result.data.id);
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<Field.FieldGroup>
	<Field.Field>
		<Field.FieldLabel for={nameInputId}>Name</Field.FieldLabel>
		<Input id={nameInputId} bind:value={name} />
	</Field.Field>
	<Field.Field>
		<Field.FieldLabel for={descriptionInputId}>Description</Field.FieldLabel>
		<Textarea id={descriptionInputId} bind:value={description} />
	</Field.Field>
</Field.FieldGroup>
{#if createProject.error}
	<p class="text-sm text-destructive">{createProject.error.message}</p>
{/if}
<div class="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
	<Button variant="outline" onclick={onCancel}>Cancel</Button>
	<Button disabled={!name.trim() || createProject.isPending} onclick={submitProject}
		>Create</Button
	>
</div>
