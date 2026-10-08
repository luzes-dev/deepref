<script lang="ts">
	import * as Field from '@deepref/ui/field';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Textarea } from '@deepref/ui/textarea';
	import { Spinner } from '@deepref/ui/spinner';
	import { createCreateProject } from '$lib/api/generated/projects/projects';
	import { notifyError } from '$lib/features/notifications/toast';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { formatProjectDate, projectsNamed } from '../project-search';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

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

	const workspace = useProjectWorkspaceContext();
	const createProject = createCreateProject();
	const duplicateHintId = $derived(`${nameInputId}-duplicate`);

	let name = $state('');
	let description = $state('');

	// A soft hint only: duplicate names are allowed, because two reviews can
	// legitimately share a working title.
	const sameName = $derived(projectsNamed(workspace.projects, name));
	const duplicateHint = $derived(
		sameName.length === 0
			? ''
			: sameName.length === 1
				? `A project with this name already exists, created ${formatProjectDate(sameName[0].created_at)}. You can still create this one.`
				: `${sameName.length} projects with this name already exist. You can still create this one.`
	);

	async function submitProject() {
		try {
			// No depth is sent: the server gives the new project the workspace Settings default.
			const result = await createProject.mutateAsync({
				data: {
					name: name.trim(),
					description: description.trim()
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
				aria-describedby={duplicateHint ? duplicateHintId : undefined}
				data-testid="create-project-name"
			/>
			{#if duplicateHint}
				<p
					id={duplicateHintId}
					role="status"
					class="flex items-start gap-2 text-xs text-warning"
					data-testid="create-project-duplicate-hint"
				>
					<TriangleAlertIcon class="mt-0.5 size-3.5 shrink-0" aria-hidden="true" />
					<span>{duplicateHint}</span>
				</p>
			{/if}
		</Field.Field>
		<Field.Field>
			<Field.FieldLabel for={descriptionInputId}>Description</Field.FieldLabel>
			<Textarea
				id={descriptionInputId}
				bind:value={description}
				placeholder="Review question or scope (optional)"
			/>
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
