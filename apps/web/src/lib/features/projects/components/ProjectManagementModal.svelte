<script lang="ts">
	import * as Empty from '@deepref/ui/empty';
	import * as Modal from '@deepref/ui/modal';
	import { Badge } from '@deepref/ui/badge';
	import { Input } from '@deepref/ui/input';
	import { ScrollArea } from '@deepref/ui/scroll-area';
	import { untrack } from 'svelte';
	import FoldersIcon from '@lucide/svelte/icons/folders';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { filterProjects } from '../project-search';
	import ProjectManagementItem from './ProjectManagementItem.svelte';

	const workspace = useProjectWorkspaceContext();
	let query = $state('');
	const matches = $derived(filterProjects(workspace.projects, query));
	const trimmedQuery = $derived(query.trim());

	// Search must see every project, so fetch the remaining pages while the
	// modal is open.
	$effect(() => {
		if (
			workspace.projectManagementOpen &&
			workspace.projectsHasNextPage &&
			!workspace.projectsLoadingMore
		) {
			untrack(() => workspace.loadMoreProjects());
		}
	});
</script>

<Modal.Root bind:open={workspace.projectManagementOpen}>
	<Modal.Content
		class="h-[80svh] max-h-[80svh] overflow-hidden border-primary/20 bg-background sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-2xl"
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
				<Badge variant="outline"
					>{trimmedQuery
						? `${matches.length} of ${workspace.projects.length}`
						: `${workspace.projects.length} projects`}</Badge
				>
			</div>
			<Modal.Description>
				Update project details or remove projects from the workspace.
			</Modal.Description>
			{#if workspace.projects.length > 0}
				<div class="relative mt-3">
					<SearchIcon
						class="pointer-events-none absolute top-1/2 left-3 size-3.5 -translate-y-1/2 text-muted-foreground"
						aria-hidden="true"
					/>
					<Input
						type="search"
						aria-label="Search projects"
						placeholder="Search by name or description"
						bind:value={query}
						class="h-9 pl-9 text-sm"
						data-testid="manage-projects-search"
					/>
				</div>
			{/if}
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
				{#if matches.length === 0}
					<p class="py-10 text-center text-sm text-muted-foreground" role="status">
						No projects match “{trimmedQuery}”.
					</p>
				{:else}
					<div class="flex flex-col gap-3">
						{#each matches as project (project.id)}
							<ProjectManagementItem {project} />
						{/each}
					</div>
				{/if}
			</ScrollArea>
		{/if}
	</Modal.Content>
</Modal.Root>
