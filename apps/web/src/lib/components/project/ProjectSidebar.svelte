<script lang="ts">
	import { resolve } from '$app/paths';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import type { ProjectWorkspaceNavView } from './types';
	import type { LucideIcon } from '@lucide/svelte';
	import ArchiveIcon from '@lucide/svelte/icons/archive';
	import ClipboardCheckIcon from '@lucide/svelte/icons/clipboard-check';
	import ClipboardListIcon from '@lucide/svelte/icons/clipboard-list';
	import ClipboardPenLineIcon from '@lucide/svelte/icons/clipboard-pen-line';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import GitForkIcon from '@lucide/svelte/icons/git-fork';
	import GitCompareIcon from '@lucide/svelte/icons/git-compare';
	import HomeIcon from '@lucide/svelte/icons/home';
	import LightbulbIcon from '@lucide/svelte/icons/lightbulb';
	import TablePropertiesIcon from '@lucide/svelte/icons/table-properties';
	import ProjectSelector from './ProjectSelector.svelte';
	import { cn } from '$lib/utils';
	import { Button, buttonVariants } from '$lib/components/ui/button';

	let {
		collapsed = false
	}: {
		collapsed?: boolean;
	} = $props();

	const workspace = useProjectWorkspaceContext();
	const projectItems: ReadonlyArray<{
		label: string;
		path:
			| '/projects/[projectId]/studies'
			| '/projects/[projectId]/appraisal'
			| '/projects/[projectId]/extraction';
		icon: LucideIcon;
	}> = [
		{ label: 'Studies', path: '/projects/[projectId]/studies', icon: ClipboardListIcon },
		{ label: 'Appraisal', path: '/projects/[projectId]/appraisal', icon: ClipboardPenLineIcon },
		{ label: 'Extraction', path: '/projects/[projectId]/extraction', icon: TablePropertiesIcon }
	];
	const navItems: {
		view: ProjectWorkspaceNavView;
		label: string;
		icon: LucideIcon;
		count?: number;
	}[] = $derived([
		{ view: 'overview', label: 'Overview', icon: HomeIcon },
		{ view: 'protocol', label: 'Protocol', icon: ClipboardListIcon },
		{ view: 'prisma', label: 'PRISMA', icon: ClipboardCheckIcon },
		{
			view: 'articles',
			label: 'Articles',
			icon: FileTextIcon,
			count: workspace.counts.articles
		},
		{ view: 'graph', label: 'Graph', icon: GitForkIcon },
		{
			view: 'recommendations',
			label: 'Recommendations',
			icon: LightbulbIcon,
			count: workspace.counts.recommendations
		},
		{
			view: 'ingestions',
			label: 'Ingestions',
			icon: ArchiveIcon,
			count: workspace.counts.ingestions
		}
	]);
	const screeningHref = $derived(
		workspace.selectedProjectId
			? resolve('/projects/[projectId]/screening/title-abstract', {
					projectId: workspace.selectedProjectId
				})
			: undefined
	);
</script>

<Tooltip.Provider>
	<aside class="flex h-full flex-col border-r bg-background">
		<div class="flex items-center justify-between gap-2 border-b p-3">
			<ProjectSelector isCollapsed={collapsed} />
		</div>

		<ScrollArea
			data-collapsed={collapsed}
			class="group flex flex-col gap-4 py-2 data-[collapsed=true]:py-2"
		>
			<nav
				class="grid gap-1 px-2 group-data-[collapsed=true]:justify-center group-data-[collapsed=true]:px-2"
			>
				{#each navItems as item (item.view)}
					{@const Icon = item.icon}
					{#if collapsed}
						<Tooltip.Root>
							<Tooltip.Trigger
								class={cn(
									buttonVariants({
										variant: workspace.view === item.view ? 'default' : 'ghost',
										size: 'icon',
										class: 'size-9'
									}),
									workspace.view === item.view &&
										'dark:bg-muted dark:text-muted-foreground dark:hover:bg-muted dark:hover:text-white'
								)}
								onclick={() => workspace.selectView(item.view)}
							>
								<Icon class="size-4" aria-hidden={true} />
								<span class="sr-only">{item.label}</span>
							</Tooltip.Trigger>
							<Tooltip.Content side="right" class="flex items-center gap-4">
								{item.label}
								{#if item.count}
									<span class="ml-auto text-muted-foreground">
										{item.count}
									</span>
								{/if}
							</Tooltip.Content>
						</Tooltip.Root>
					{:else}
						<Button
							variant={workspace.view === item.view ? 'default' : 'ghost'}
							size="sm"
							class={cn('justify-start', {
								'dark:bg-muted dark:text-white dark:hover:bg-muted dark:hover:text-white':
									workspace.view === item.view
							})}
							onclick={() => workspace.selectView(item.view)}
						>
							<Icon class="mr-2 size-4" aria-hidden={true} />
							{item.label}
							{#if item.count}
								<span
									class={cn('ml-auto', {
										'text-background dark:text-white':
											workspace.view === item.view
									})}
								>
									{item.count}
								</span>
							{/if}
						</Button>
					{/if}
				{/each}
				{#if screeningHref}
					{#if collapsed}
						<a
							href={resolve('/projects/[projectId]/screening/title-abstract', {
								projectId: workspace.selectedProjectId
							})}
							title="Screening"
							class={cn(
								buttonVariants({
									variant: 'outline',
									size: 'icon',
									class: 'size-9'
								})
							)}
						>
							<ClipboardCheckIcon aria-hidden={true} />
							<span class="sr-only">Screening</span>
						</a>
					{:else}
						<a
							href={resolve('/projects/[projectId]/screening/title-abstract', {
								projectId: workspace.selectedProjectId
							})}
							class={cn(
								buttonVariants({
									variant: 'outline',
									size: 'sm',
									class: 'justify-start'
								})
							)}
						>
							<ClipboardCheckIcon class="mr-2 size-4" aria-hidden={true} />
							Screening
						</a>
					{/if}
				{/if}
				{#if workspace.selectedProjectId}
					{#each projectItems as projectItem (projectItem.label)}
						{@const ProjectIcon = projectItem.icon}
						<a
							href={resolve(projectItem.path, {
								projectId: workspace.selectedProjectId
							})}
							title={projectItem.label}
							class={cn(
								buttonVariants({
									variant: 'outline',
									size: collapsed ? 'icon' : 'sm',
									class: collapsed ? 'size-9' : 'justify-start'
								})
							)}
						>
							<ProjectIcon
								class={collapsed ? 'size-4' : 'mr-2 size-4'}
								aria-hidden={true}
							/>
							{#if collapsed}<span class="sr-only">{projectItem.label}</span
								>{:else}{projectItem.label}{/if}
						</a>
					{/each}
				{/if}
				{#if workspace.selectedProjectId}
					{#if collapsed}
						<a
							href={resolve('/projects/[projectId]/discovery/duplicates', {
								projectId: workspace.selectedProjectId
							})}
							title="Deduplication"
							class={cn(
								buttonVariants({
									variant: 'outline',
									size: 'icon',
									class: 'size-9'
								})
							)}
						>
							<GitCompareIcon aria-hidden={true} />
							<span class="sr-only">Deduplication</span>
						</a>
					{:else}
						<a
							href={resolve('/projects/[projectId]/discovery/duplicates', {
								projectId: workspace.selectedProjectId
							})}
							class={cn(
								buttonVariants({
									variant: 'outline',
									size: 'sm',
									class: 'justify-start'
								})
							)}
						>
							<GitCompareIcon class="mr-2 size-4" aria-hidden={true} />
							Deduplication
						</a>
					{/if}
				{/if}
			</nav>
		</ScrollArea>
	</aside>
</Tooltip.Provider>
