<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import { cn } from '$lib/utils';
	import { Button, buttonVariants } from '$lib/components/ui/button';
	import * as Sheet from '$lib/components/ui/sheet';
	import MenuIcon from '@lucide/svelte/icons/menu';
	import SettingsIcon from '@lucide/svelte/icons/settings-2';
	import ArticleInspector from './ArticleInspector.svelte';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSelector from './ProjectSelector.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';
	import {
		PROJECT_NAVIGATION_GROUPS,
		isProjectNavigationItemActive,
		type ProjectNavigationItem
	} from './navigation';

	let { children }: { children?: Snippet } = $props();

	const workspace = useProjectWorkspaceContext();
	const projectId = $derived(workspace.selectedProjectId);
	const pathname = $derived(page.url.pathname);
	let menuOpen = $state(false);
	const currentRoute = $derived.by(() => {
		for (const group of PROJECT_NAVIGATION_GROUPS) {
			const item = group.items.find((candidate) => isActive(candidate));
			if (item) return item;
		}
		return undefined;
	});
	const mobileViewLabel = $derived(currentRoute?.label ?? 'Overview');
	const mobileViewDescription = $derived(
		currentRoute?.description ?? 'A working summary of the evidence workspace.'
	);
	const articleSheetOpen = $derived(
		Boolean(workspace.selectedArticle) &&
			(workspace.view === 'articles' ||
				workspace.view === 'graph' ||
				workspace.view === 'recommendations')
	);
	const ingestionSheetOpen = $derived(Boolean(workspace.selectedIngestion));

	function isActive(item: ProjectNavigationItem): boolean {
		return Boolean(projectId) && isProjectNavigationItemActive(item, pathname, projectId);
	}

	function choose(item: ProjectNavigationItem): void {
		if (item.view) workspace.selectView(item.view);
		menuOpen = false;
	}
</script>

<div class="flex h-full flex-col">
	<div class="flex flex-col gap-3 border-b border-border/70 p-3">
		<div class="flex items-center gap-2">
			<Button
				variant="ghost"
				size="icon"
				aria-label="Open navigation"
				data-testid="mobile-navigation-trigger"
				onclick={() => (menuOpen = true)}
			>
				<MenuIcon data-icon aria-hidden="true" />
			</Button>
			<div class="min-w-0 flex-1">
				<p class="text-xs font-bold tracking-[0.16em] text-primary uppercase">DeepRef</p>
				<p
					class="truncate text-sm font-medium text-foreground"
					data-testid="mobile-current-route"
					aria-live="polite"
				>
					{mobileViewLabel}
				</p>
			</div>
		</div>
		<div class="min-w-0">
			<ProjectSelector />
		</div>
		<p class="truncate text-xs text-muted-foreground">{mobileViewDescription}</p>
	</div>
	<div class="min-h-0 flex-1 overflow-auto">
		<ProjectWorkspaceViewPanel {children} />
	</div>

	<Sheet.Root bind:open={menuOpen}>
		<Sheet.Content
			side="left"
			class="w-[min(21rem,calc(100vw-2rem))] gap-0 p-0"
			data-testid="mobile-navigation-sheet"
		>
			<Sheet.Header class="border-b border-border/70 p-5 text-left">
				<Sheet.Title class="editorial-title text-2xl">Evidence atelier</Sheet.Title>
				<Sheet.Description>Move through the review workflow.</Sheet.Description>
			</Sheet.Header>
			<nav class="min-h-0 flex-1 overflow-y-auto p-4" aria-label="Mobile evidence workflow">
				<div class="grid gap-5">
					{#each PROJECT_NAVIGATION_GROUPS as group (group.id)}
						<section
							class="grid gap-1"
							aria-labelledby={`mobile-nav-group-${group.id}`}
						>
							<h2
								id={`mobile-nav-group-${group.id}`}
								class="px-2 text-xs font-semibold tracking-[0.14em] text-muted-foreground uppercase"
							>
								{group.label}
							</h2>
							{#each group.items as item (item.id)}
								{@const Icon = item.icon}
								{@const active = isActive(item)}
								{#if item.view}
									<Button
										variant={active ? 'secondary' : 'ghost'}
										size="sm"
										class={cn('justify-start', active && 'text-primary')}
										onclick={() => choose(item)}
										aria-current={active ? 'page' : undefined}
									>
										<Icon data-icon="inline-start" aria-hidden="true" />
										{item.label}
									</Button>
								{:else if projectId}
									<a
										href={resolve(item.path, { projectId })}
										class={cn(
											buttonVariants({
												variant: active ? 'secondary' : 'ghost',
												size: 'sm',
												class: 'justify-start'
											}),
											active && 'text-primary'
										)}
										onclick={() => (menuOpen = false)}
										aria-current={active ? 'page' : undefined}
									>
										<Icon data-icon="inline-start" aria-hidden="true" />
										{item.label}
									</a>
								{/if}
							{/each}
						</section>
					{/each}
				</div>
			</nav>
			<div class="border-t border-border/70 p-4">
				<a
					href={resolve('/settings')}
					class={buttonVariants({
						variant: 'ghost',
						size: 'sm',
						class: 'w-full justify-start'
					})}
					onclick={() => (menuOpen = false)}
				>
					<SettingsIcon data-icon="inline-start" aria-hidden="true" />
					Settings
				</a>
			</div>
		</Sheet.Content>
	</Sheet.Root>

	<Sheet.Root open={articleSheetOpen} onOpenChange={(open) => !open && workspace.clearArticle()}>
		<Sheet.Content side="right" class="w-full p-0">
			<Sheet.Header class="sr-only">
				<Sheet.Title>Article inspector</Sheet.Title>
			</Sheet.Header>
			<ArticleInspector />
		</Sheet.Content>
	</Sheet.Root>
	<Sheet.Root
		open={ingestionSheetOpen}
		onOpenChange={(open) => !open && workspace.clearIngestion()}
	>
		<Sheet.Content side="right" class="w-full p-0">
			<Sheet.Header class="sr-only">
				<Sheet.Title>Ingestion inspector</Sheet.Title>
			</Sheet.Header>
			<IngestionInspector />
		</Sheet.Content>
	</Sheet.Root>
</div>
