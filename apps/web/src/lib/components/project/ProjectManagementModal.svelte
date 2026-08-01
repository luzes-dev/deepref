<script lang="ts">
	import * as Empty from '$lib/components/ui/empty';
	import * as Modal from '$lib/components/ui/modal';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import ProjectManagementItem from './ProjectManagementItem.svelte';

	const workspace = useProjectWorkspaceContext();
</script>

<Modal.Root bind:open={workspace.projectManagementOpen}>
	<Modal.Content class="h-[80svh] max-h-[80svh] overflow-hidden sm:h-auto sm:max-w-2xl">
		<Modal.Header>
			<Modal.Title>Manage projects</Modal.Title>
			<Modal.Description>
				Update project details or remove projects from the workspace.
			</Modal.Description>
		</Modal.Header>

		{#if workspace.projects.length === 0}
			<Empty.Root>
				<Empty.Header>
					<Empty.Title>No projects</Empty.Title>
					<Empty.Description>There are no projects to manage.</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else}
			<ScrollArea
				class="min-h-0 flex-1 overflow-hidden px-4 pb-4 sm:max-h-[min(28rem,calc(100svh-14rem))] sm:flex-none sm:px-0 sm:pr-3 sm:pb-0"
			>
				<div class="flex flex-col gap-4">
					{#each workspace.projects as project (project.id)}
						<ProjectManagementItem {project} />
					{/each}
				</div>
			</ScrollArea>
		{/if}
	</Modal.Content>
</Modal.Root>
