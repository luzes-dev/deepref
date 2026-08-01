<script lang="ts">
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import type { ProjectWorkspaceView } from './types';
	import type { LucideIcon } from '@lucide/svelte';
	import ArchiveIcon from '@lucide/svelte/icons/archive';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import GitForkIcon from '@lucide/svelte/icons/git-fork';
	import HomeIcon from '@lucide/svelte/icons/home';
	import LightbulbIcon from '@lucide/svelte/icons/lightbulb';
	import ProjectSelector from './ProjectSelector.svelte';
	import { cn } from '$lib/utils';
	import { Button, buttonVariants } from '$lib/components/ui/button';

	let {
		collapsed = false
	}: {
		collapsed?: boolean;
	} = $props();

	const workspace = useProjectWorkspaceContext();
	const navItems: {
		view: ProjectWorkspaceView;
		label: string;
		icon: LucideIcon;
		count?: number;
	}[] = $derived([
		{ view: 'overview', label: 'Overview', icon: HomeIcon },
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
									<span class="text-muted-foreground ml-auto">
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
								'dark:bg-muted dark:hover:bg-muted dark:text-white dark:hover:text-white':
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
			</nav>
		</ScrollArea>
	</aside>
</Tooltip.Provider>
