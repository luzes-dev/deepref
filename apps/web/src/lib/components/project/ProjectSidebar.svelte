<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { cn } from '$lib/utils';
	import { ScrollArea } from '$lib/components/ui/scroll-area';
	import * as Tooltip from '$lib/components/ui/tooltip';
	import { Button, buttonVariants } from '$lib/components/ui/button';
	import SettingsIcon from '@lucide/svelte/icons/settings-2';
	import ProjectSelector from './ProjectSelector.svelte';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import {
		PROJECT_NAVIGATION_GROUPS,
		isProjectNavigationItemActive,
		type ProjectNavigationItem
	} from './navigation';

	let { collapsed = false }: { collapsed?: boolean } = $props();

	const workspace = useProjectWorkspaceContext();
	const pathname = $derived(page.url.pathname);
	const projectId = $derived(workspace.selectedProjectId);

	function isActive(item: ProjectNavigationItem): boolean {
		return Boolean(projectId) && isProjectNavigationItemActive(item, pathname, projectId);
	}

	function choose(item: ProjectNavigationItem): void {
		if (item.view) workspace.selectView(item.view);
	}
</script>

<Tooltip.Provider>
	<aside
		class="flex h-full min-h-0 flex-col border-r bg-sidebar text-sidebar-foreground"
		aria-label="Project navigation"
		data-testid="project-sidebar"
	>
		<div class="flex items-center justify-between gap-2 border-b border-sidebar-border p-3">
			<div class={cn('min-w-0', collapsed && 'w-full')}>
				<div class={cn('mb-2 flex items-center gap-2', collapsed && 'justify-center')}>
					<span
						class="grid size-7 shrink-0 place-items-center rounded-md bg-primary text-xs font-bold text-primary-foreground"
						aria-hidden="true">D</span
					>
					<span
						class={cn(
							'truncate text-xs font-bold tracking-[0.16em] text-sidebar-foreground',
							collapsed && 'sr-only'
						)}>DEEPREF</span
					>
				</div>
				<ProjectSelector isCollapsed={collapsed} />
			</div>
		</div>

		<ScrollArea
			data-collapsed={collapsed}
			class="group min-h-0 flex-1 py-3 data-[collapsed=true]:py-2"
		>
			<nav
				class="grid gap-4 px-2 group-data-[collapsed=true]:justify-center group-data-[collapsed=true]:gap-2 group-data-[collapsed=true]:px-2"
				aria-label="Evidence workflow"
			>
				{#each PROJECT_NAVIGATION_GROUPS as group (group.id)}
					<section class="grid gap-1" aria-labelledby={`nav-group-${group.id}`}>
						<h2
							id={`nav-group-${group.id}`}
							class={cn(
								'px-2 text-[0.65rem] font-semibold tracking-[0.16em] text-muted-foreground uppercase',
								collapsed && 'sr-only'
							)}
						>
							{group.label}
						</h2>
						{#each group.items as item (item.id)}
							{@const Icon = item.icon}
							{@const active = isActive(item)}
							{#if item.view}
								{#if collapsed}
									<Tooltip.Root>
										<Tooltip.Trigger
											class={cn(
												buttonVariants({
													variant: active ? 'default' : 'ghost',
													size: 'icon',
													class: 'size-9'
												}),
												active &&
													'bg-sidebar-primary text-sidebar-primary-foreground'
											)}
											onclick={() => choose(item)}
											aria-current={active ? 'page' : undefined}
										>
											<Icon data-icon aria-hidden="true" />
											<span class="sr-only">{item.label}</span>
										</Tooltip.Trigger>
										<Tooltip.Content side="right">{item.label}</Tooltip.Content>
									</Tooltip.Root>
								{:else}
									<Button
										variant={active ? 'default' : 'ghost'}
										size="sm"
										class={cn(
											'justify-start text-sidebar-foreground',
											active &&
												'bg-sidebar-primary text-sidebar-primary-foreground hover:bg-sidebar-primary/90'
										)}
										onclick={() => choose(item)}
										aria-current={active ? 'page' : undefined}
									>
										<Icon data-icon="inline-start" aria-hidden="true" />
										{item.label}
										{#if item.id === 'articles'}<span class="ml-auto text-xs"
												>{workspace.counts.articles ?? 0}</span
											>{/if}
										{#if item.id === 'recommendations'}<span
												class="ml-auto text-xs"
												>{workspace.counts.recommendations ?? 0}</span
											>{/if}
										{#if item.id === 'imports'}<span class="ml-auto text-xs"
												>{workspace.counts.ingestions ?? 0}</span
											>{/if}
									</Button>
								{/if}
							{:else if projectId}
								{#if collapsed}
									<a
										href={resolve(item.path, { projectId })}
										title={item.label}
										class={cn(
											buttonVariants({
												variant: active ? 'secondary' : 'ghost',
												size: 'icon',
												class: 'size-9'
											}),
											active && 'text-sidebar-accent-foreground'
										)}
										aria-current={active ? 'page' : undefined}
									>
										<Icon data-icon aria-hidden="true" />
										<span class="sr-only">{item.label}</span>
									</a>
								{:else}
									<a
										href={resolve(item.path, { projectId })}
										class={cn(
											buttonVariants({
												variant: active ? 'secondary' : 'ghost',
												size: 'sm',
												class: 'justify-start'
											}),
											active && 'text-sidebar-accent-foreground'
										)}
										aria-current={active ? 'page' : undefined}
									>
										<Icon data-icon="inline-start" aria-hidden="true" />
										{item.label}
									</a>
								{/if}
							{/if}
						{/each}
					</section>
				{/each}
			</nav>
		</ScrollArea>

		<footer class="border-t border-sidebar-border p-2">
			{#if collapsed}
				<a
					href={resolve('/settings')}
					title="Settings"
					class={buttonVariants({
						variant: 'ghost',
						size: 'icon',
						class: 'size-9 w-full'
					})}
				>
					<SettingsIcon data-icon aria-hidden="true" />
					<span class="sr-only">Settings</span>
				</a>
			{:else}
				<a
					href={resolve('/settings')}
					class={buttonVariants({
						variant: 'ghost',
						size: 'sm',
						class: 'w-full justify-start'
					})}
				>
					<SettingsIcon data-icon="inline-start" aria-hidden="true" />
					Settings
				</a>
			{/if}
		</footer>
	</aside>
</Tooltip.Provider>
