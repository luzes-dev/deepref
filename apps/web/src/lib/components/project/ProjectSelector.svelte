<script lang="ts">
	import * as Command from '$lib/components/ui/command';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Drawer from '$lib/components/ui/drawer';
	import * as Popover from '$lib/components/ui/popover';
	import { Button } from '$lib/components/ui/button';
	import { IsMobile } from '$lib/hooks/is-mobile.svelte';
	import { cn } from '$lib/utils';
	import type { ComponentProps } from 'svelte';
	import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
	import FoldersIcon from '@lucide/svelte/icons/folders';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import ProjectCreateForm from './ProjectCreateForm.svelte';

	type TriggerProps = ComponentProps<typeof Button>;

	let {
		isCollapsed
	}: {
		isCollapsed?: boolean;
	} = $props();

	const isMobile = new IsMobile();
	const workspace = useProjectWorkspaceContext();

	const selectedProject = $derived(
		workspace.projects.find((project) => project.id === workspace.selectedProjectId) ??
			workspace.projects[0]
	);
</script>

{#snippet trigger(props: TriggerProps)}
	<Button
		{...props}
		variant="ghost"
		role="combobox"
		aria-expanded={workspace.projectSelectorOpen}
		aria-label="Select project"
		class={cn(
			'w-full justify-between gap-2 border-accent',
			isCollapsed && 'size-9 shrink-0 justify-center p-0'
		)}
	>
		<span class="flex min-w-0 items-center gap-2">
			<FoldersIcon />
			<span class={cn('truncate', isCollapsed && 'sr-only')}>
				{selectedProject?.name ?? 'Select project'}
			</span>
		</span>
		<ChevronsUpDownIcon class={cn('opacity-50', isCollapsed && 'hidden')} />
	</Button>
{/snippet}

{#snippet projectCommand()}
	<Command.Root>
		<Command.Input placeholder="Search projects..." />
		<Command.List>
			<Command.Empty>No projects found.</Command.Empty>
			<Command.Group class="max-h-48 overflow-y-auto pr-1">
				{#each workspace.projects as project (project.id)}
					<Command.Item
						value={project.id}
						onSelect={() => workspace.selectProjectFromSelector(project.id)}
						data-checked={project.id === workspace.selectedProjectId}
					>
						<FoldersIcon />
						<span class="truncate">{project.name}</span>
					</Command.Item>
				{/each}
			</Command.Group>
			<Command.Separator />
			<Command.Group>
				<Command.Item value="Create project" onSelect={workspace.openCreateFromSelector}>
					<PlusIcon />
					Create project
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
		<Drawer.Content>
			<Drawer.Header>
				<Drawer.Title>Select project</Drawer.Title>
				<Drawer.Description
					>Search existing projects or create a new one.</Drawer.Description
				>
			</Drawer.Header>
			<div class="border-t">
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
		<Popover.Content class="w-72 p-0" align="start">
			{@render projectCommand()}
		</Popover.Content>
	</Popover.Root>
{/if}

<Dialog.Root bind:open={workspace.projectCreateOpen}>
	<Dialog.Content>
		<Dialog.Header>
			<Dialog.Title>Create project</Dialog.Title>
			<Dialog.Description>Define a research workspace for DOI ingestion.</Dialog.Description>
		</Dialog.Header>
		<ProjectCreateForm
			nameInputId="selector-project-name"
			descriptionInputId="selector-project-description"
			onCreated={workspace.finishProjectCreated}
			onCancel={workspace.closeProjectCreate}
		/>
	</Dialog.Content>
</Dialog.Root>
