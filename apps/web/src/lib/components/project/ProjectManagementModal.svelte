<script lang="ts">
	import * as Empty from '$lib/components/ui/empty';
	import * as Modal from '$lib/components/ui/modal';
	import { Badge } from '$lib/components/ui/badge';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import FoldersIcon from '@lucide/svelte/icons/folders';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import ProjectManagementItem from './ProjectManagementItem.svelte';

	const workspace = useProjectWorkspaceContext();
</script>

<Modal.Root bind:open={workspace.projectManagementOpen}>
	<Modal.Content
		class="h-[80svh] max-h-[80svh] overflow-hidden border-primary/20 bg-background sm:h-auto sm:max-w-2xl"
	>
		<Modal.Header class="border-b border-border/70 pb-4">
			<div class="flex items-center justify-between gap-3 pr-8">
				<div class="flex min-w-0 items-center gap-2">
					<span
						class="flex size-8 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
					>
						<FoldersIcon aria-hidden="true" />
					</span>
					<Modal.Title>Manage projects</Modal.Title>
				</div>
				<Badge variant="outline">{workspace.projects.length} projects</Badge>
			</div>
			<Modal.Description>
				Update project details or remove projects from the workspace.
			</Modal.Description>
		</Modal.Header>

		{#if workspace.projects.length === 0}
			<Empty.Root class="border-0 py-12">
				<Empty.Media variant="icon"><FoldersIcon aria-hidden="true" /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No projects</Empty.Title>
					<Empty.Description>There are no projects to manage.</Empty.Description>
				</Empty.Header>
			</Empty.Root>
		{:else}
			<ScrollArea
				class="min-h-0 flex-1 overflow-hidden px-4 pb-4 sm:max-h-[min(27.25rem,calc(100svh-14.75rem))] sm:flex-none sm:px-0 sm:pr-3 sm:pb-0"
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
