<script lang="ts">
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { cn } from '$lib/utils';
	import { ScrollArea } from '@deepref/ui/scroll-area';
	import * as Tooltip from '@deepref/ui/tooltip';
	import { buttonVariants } from '@deepref/ui/button';
	import SettingsIcon from '@lucide/svelte/icons/settings-2';
	import ProjectSelector from './ProjectSelector.svelte';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import {
		PROJECT_NAVIGATION_GROUPS,
		isProjectNavigationItemActive,
		type ProjectNavigationItem
	} from '../navigation';

	let { collapsed = false }: { collapsed?: boolean } = $props();

	const workspace = useProjectWorkspaceContext();
	const pathname = $derived(page.url.pathname);
	const projectId = $derived(workspace.selectedProjectId);

	function choose(event: MouseEvent, item: ProjectNavigationItem): void {
		if (
			!item.view ||
			event.button !== 0 ||
			event.metaKey ||
			event.ctrlKey ||
			event.shiftKey ||
			event.altKey
		)
			return;
		event.preventDefault();
		workspace.selectView(item.view);
	}

	function isActive(item: ProjectNavigationItem): boolean {
		return Boolean(projectId) && isProjectNavigationItemActive(item, pathname, projectId);
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
							'truncate text-base font-semibold tracking-tight text-sidebar-foreground',
							collapsed && 'sr-only'
						)}>DeepRef</span
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
				class="grid gap-3 px-3 group-data-[collapsed=true]:justify-center group-data-[collapsed=true]:gap-2 group-data-[collapsed=true]:px-2"
				aria-label="Evidence workflow"
			>
				{#each PROJECT_NAVIGATION_GROUPS as group (group.id)}
					<section class="grid gap-0.5" aria-labelledby={`nav-group-${group.id}`}>
						<h2
							id={`nav-group-${group.id}`}
							class={cn(
								'px-2 pb-1 text-xs font-medium text-muted-foreground',
								collapsed && 'sr-only'
							)}
						>
							{group.label}
						</h2>
						{#each group.items as item (item.id)}
							{@const Icon = item.icon}
							{@const active = isActive(item)}
							{#if projectId}
								<a
									href={resolve(item.path, { projectId })}
									onclick={(event) => choose(event, item)}
									title={collapsed ? item.label : item.description}
									class={cn(
										buttonVariants({
											variant: active ? 'secondary' : 'ghost',
											size: 'sm'
										}),
										'nav-link',
										collapsed && 'nav-link-collapsed'
									)}
									aria-current={active ? 'page' : undefined}
								>
									<Icon data-icon="inline-start" aria-hidden="true" />
									<span class={collapsed ? 'sr-only' : ''}>{item.label}</span>
									{#if !collapsed && item.id === 'articles'}<span
											class="ml-auto text-xs text-muted-foreground tabular-nums"
											>{workspace.counts.articles ?? 0}</span
										>{/if}
								</a>
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

<style>
	.nav-link {
		width: 100%;
		justify-content: flex-start;
		gap: 0.625rem;
		font-weight: 400;
	}
	.nav-link[aria-current='page'] {
		font-weight: 600;
	}
	.nav-link-collapsed {
		width: 2.25rem;
		justify-content: center;
	}
</style>
