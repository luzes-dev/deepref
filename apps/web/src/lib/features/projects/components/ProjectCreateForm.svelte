<script lang="ts">
	import * as Field from '@deepref/ui/field';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Textarea } from '@deepref/ui/textarea';
	import { Spinner } from '@deepref/ui/spinner';
	import { createCreateProject } from '$lib/api/generated/projects/projects';
	import { notifyError } from '$lib/features/notifications/toast';
	import { DEFAULT_PROJECT_MAX_DEPTH } from '../constants';
	import PlusIcon from '@lucide/svelte/icons/plus';

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
		} catch (error) {
			notifyError('Project could not be created', error);
		}
	}
</script>

<form
	class="flex flex-col gap-5"
	onsubmit={(event) => {
		event.preventDefault();
		void submitProject();
	}}
>
	<Field.FieldGroup class="gap-5">
		<Field.Field data-invalid={name.length > 0 && !name.trim()}>
			<Field.FieldLabel for={nameInputId}>Name</Field.FieldLabel>
			<Input
				id={nameInputId}
				bind:value={name}
				required
				aria-invalid={name.length > 0 && !name.trim()}
			/>
			<Field.FieldDescription
				>A short name for this evidence workspace.</Field.FieldDescription
			>
		</Field.Field>
		<Field.Field>
			<Field.FieldLabel for={descriptionInputId}>Description</Field.FieldLabel>
			<Textarea id={descriptionInputId} bind:value={description} />
			<Field.FieldDescription>Optional context for collaborators.</Field.FieldDescription>
		</Field.Field>
	</Field.FieldGroup>
	<div
		class="flex flex-col-reverse gap-2 border-t border-border/70 pt-4 sm:flex-row sm:justify-end"
	>
		<Button type="button" variant="outline" onclick={onCancel}>Cancel</Button>
		<Button type="submit" disabled={!name.trim() || createProject.isPending}>
			{#if createProject.isPending}<Spinner data-icon="inline-start" />{/if}
			<PlusIcon data-icon="inline-start" aria-hidden="true" />Create
		</Button>
	</div>
</form>
