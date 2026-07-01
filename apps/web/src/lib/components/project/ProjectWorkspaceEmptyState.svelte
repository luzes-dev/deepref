<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import ProjectCreateForm from './ProjectCreateForm.svelte';

	const workspace = useProjectWorkspaceContext();
</script>

<div class="flex h-full items-center justify-center p-4">
	<Empty.Root>
		<Empty.Header>
			<Empty.Title>No projects</Empty.Title>
			<Empty.Description>
				Create a project from the workspace sidebar to start ingesting DOIs.
			</Empty.Description>
		</Empty.Header>
		<Empty.Content>
			<Dialog.Root bind:open={workspace.projectCreateOpen}>
				<Dialog.Trigger>
					{#snippet child({ props })}
						<Button {...props}
							><PlusIcon data-icon="inline-start" />Create project</Button
						>
					{/snippet}
				</Dialog.Trigger>
				<Dialog.Content>
					<Dialog.Header>
						<Dialog.Title>Create project</Dialog.Title>
						<Dialog.Description
							>Define a research workspace for DOI ingestion.</Dialog.Description
						>
					</Dialog.Header>
					<ProjectCreateForm
						nameInputId="empty-project-name"
						descriptionInputId="empty-project-description"
						onCreated={workspace.finishProjectCreated}
						onCancel={workspace.closeProjectCreate}
					/>
				</Dialog.Content>
			</Dialog.Root>
		</Empty.Content>
	</Empty.Root>
</div>
