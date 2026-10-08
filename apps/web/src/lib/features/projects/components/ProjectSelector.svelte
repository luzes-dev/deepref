<script lang="ts">
	import * as Command from '@deepref/ui/command';
	import * as Dialog from '@deepref/ui/dialog';
	import * as Drawer from '@deepref/ui/drawer';
	import * as Popover from '@deepref/ui/popover';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { IsMobile } from '$lib/hooks/is-mobile.svelte';
	import { cn } from '$lib/utils';
	import { untrack } from 'svelte';
	import type { ComponentProps } from 'svelte';
	import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
	import FoldersIcon from '@lucide/svelte/icons/folders';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import SettingsIcon from '@lucide/svelte/icons/settings';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { projectSubtitle } from '../project-search';
	import ProjectCreateForm from './ProjectCreateForm.svelte';
	import ProjectManagementModal from './ProjectManagementModal.svelte';

	type TriggerProps = ComponentProps<typeof Button>;

	let {
		isCollapsed,
		compact = false
	}: {
		isCollapsed?: boolean;
		compact?: boolean;
	} = $props();

	const isMobile = new IsMobile();
	const workspace = useProjectWorkspaceContext();
	let searchText = $state('');

	const selectedProject = $derived(
		workspace.projects.find((project) => project.id === workspace.selectedProjectId) ??
			workspace.projects[0]
	);

	// Browsing keeps the explicit "Load more" button. Searching needs every
	// project, so the remaining pages are fetched as soon as the user types.
	$effect(() => {
		if (
			workspace.projectSelectorOpen &&
			searchText.trim() &&
			workspace.projectsHasNextPage &&
			!workspace.projectsLoadingMore
		) {
			untrack(() => workspace.loadMoreProjects());
		}
	});

	$effect(() => {
		if (!workspace.projectSelectorOpen) searchText = '';
	});
</script>

{#snippet trigger(props: TriggerProps)}
	<Button
		{...props}
		variant="ghost"
		role="combobox"
		aria-expanded={workspace.projectSelectorOpen}
		aria-label="Select project"
		class={cn(
			'w-full justify-between gap-2 border border-border/70 bg-background/70 shadow-xs hover:border-primary/40 hover:bg-muted/40',
			compact && 'w-auto max-w-40 shrink-0',
			isCollapsed && 'size-9 shrink-0 justify-center p-0'
		)}
	>
		<span class="flex min-w-0 items-center gap-2">
			<FoldersIcon data-icon="inline-start" aria-hidden="true" />
			<span class={cn('truncate', isCollapsed && 'sr-only')}>
				{selectedProject?.name ?? 'Select project'}
			</span>
		</span>
		<ChevronsUpDownIcon
			data-icon="inline-end"
			aria-hidden="true"
			class={cn('opacity-50', isCollapsed && 'hidden')}
		/>
	</Button>
{/snippet}

{#snippet projectCommand()}
	<Command.Root class="min-w-0">
		<Command.Input placeholder="Search projects..." bind:value={searchText} />
		<Command.List>
			<Command.Empty class="p-4 text-sm">No projects found.</Command.Empty>
			<Command.Group class="max-h-60 overflow-y-auto px-1 pb-1">
				{#each workspace.projects as project (project.id)}
					<Command.Item
						value={project.id}
						keywords={[project.name, project.description ?? '']}
						onSelect={() => workspace.selectProjectFromSelector(project.id)}
						data-checked={project.id === workspace.selectedProjectId}
						data-testid="project-selector-item"
					>
						<FoldersIcon aria-hidden="true" />
						<span class="flex min-w-0 flex-col">
							<span class="truncate">{project.name}</span>
							<span class="truncate text-xs text-muted-foreground tabular-nums"
								>{projectSubtitle(project)}</span
							>
						</span>
					</Command.Item>
				{/each}
			</Command.Group>
			{#if workspace.projectsHasNextPage}
				<div class="border-t border-border/70 p-2">
					<Button
						variant="outline"
						size="sm"
						class="w-full"
						onclick={workspace.loadMoreProjects}
						disabled={workspace.projectsLoadingMore}
						data-testid="project-selector-load-more"
					>
						{#if workspace.projectsLoadingMore}<Spinner data-icon="inline-start" />{/if}
						{workspace.projectsLoadingMore ? 'Loading more' : 'Load more projects'}
					</Button>
				</div>
			{/if}
			<Command.Separator />
			<Command.Group class="p-1">
				<Command.Item value="Create project" onSelect={workspace.openCreateFromSelector}>
					<PlusIcon aria-hidden="true" />
					Create project
				</Command.Item>
				<Command.Item
					value="Manage projects"
					onSelect={workspace.openManagementFromSelector}
				>
					<SettingsIcon aria-hidden="true" />
					Manage projects
				</Command.Item>
			</Command.Group>
		</Command.List>
	</Command.Root>
{/snippet}

{#if isMobile.current}
	<Drawer.Root bind:open={workspace.projectSelectorOpen}>
		<Drawer.Trigger>
			{#snippet child({ props })}
				{@render trigger(props)}
			{/snippet}
		</Drawer.Trigger>
		<Drawer.Content class="max-h-[88svh] gap-0 p-0">
			<Drawer.Header class="border-b border-border/70 p-5 text-left">
				<Drawer.Title>Select project</Drawer.Title>
				<Drawer.Description
					>Search existing projects or create a new one.</Drawer.Description
				>
			</Drawer.Header>
			<div class="min-h-0 overflow-y-auto">
				{@render projectCommand()}
			</div>
		</Drawer.Content>
	</Drawer.Root>
{:else}
	<Popover.Root bind:open={workspace.projectSelectorOpen}>
		<Popover.Trigger>
			{#snippet child({ props })}
				{@render trigger(props)}
			{/snippet}
		</Popover.Trigger>
		<Popover.Content class="w-[min(22rem,calc(100vw-2rem))] p-0" align="start">
			{@render projectCommand()}
		</Popover.Content>
	</Popover.Root>
{/if}

<Dialog.Root bind:open={workspace.projectCreateOpen}>
	<Dialog.Content class="border-primary/20 sm:max-w-lg">
		<Dialog.Header class="border-b border-border/70 pb-4">
			<Dialog.Title>Create project</Dialog.Title>
			<Dialog.Description>One project per review.</Dialog.Description>
		</Dialog.Header>
		<ProjectCreateForm
			nameInputId="selector-project-name"
			descriptionInputId="selector-project-description"
			onCreated={workspace.finishProjectCreated}
			onCancel={workspace.closeProjectCreate}
		/>
	</Dialog.Content>
</Dialog.Root>

<ProjectManagementModal />
