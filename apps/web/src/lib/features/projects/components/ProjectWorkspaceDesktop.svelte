<script lang="ts">
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { resolve } from '$app/paths';
	import * as Resizable from '@deepref/ui/resizable';
	import type { PaneAPI } from 'paneforge';
	import TopNavBar from '$lib/shell/TopNavBar.svelte';
	import ArticleInspector from './ArticleInspector.svelte';
	import {
		PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID,
		PROJECT_WORKSPACE_MAIN_LAYOUT_ID
	} from '../constants';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSidebar from './ProjectSidebar.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';
	import {
		PROJECT_NAVIGATION_GROUPS,
		isProjectNavigationItemActive,
		type ProjectNavigationItem
	} from '../navigation';

	let { children }: { children?: Snippet } = $props();

	const workspace = useProjectWorkspaceContext();
	const projectId = $derived(workspace.selectedProjectId);
	const pathname = $derived(page.url.pathname);

	const hasInspector = $derived(
		(workspace.view === 'ingestions' && Boolean(workspace.selectedIngestion)) ||
			(['articles', 'graph', 'recommendations'].includes(workspace.view) &&
				Boolean(workspace.selectedArticle))
	);
	let inspectorPane = $state<PaneAPI | undefined>(undefined);

	function toggleInspector() {
		if (!inspectorPane) return;
		if (inspectorPane.isCollapsed()) {
			inspectorPane.expand();
			return;
		}

		inspectorPane.collapse();
	}

	function isActive(item: ProjectNavigationItem): boolean {
		return Boolean(projectId) && isProjectNavigationItemActive(item, pathname, projectId);
	}

	const currentRoute = $derived.by(() => {
		for (const group of PROJECT_NAVIGATION_GROUPS) {
			const item = group.items.find((candidate) => isActive(candidate));
			if (item) return item;
		}
		return undefined;
	});

	const viewTitle = $derived(currentRoute?.label ?? 'Overview');
	const projectList = $derived(workspace.projects.map((p) => ({ id: p.id, name: p.name })));
	const agentHref = $derived(
		projectId ? resolve('/projects/[projectId]/assistant', { projectId }) : undefined
	);

	function handleSelectProject(id: string | null) {
		if (id) {
			workspace.selectProject(id);
		} else {
			void goto(resolve('/'));
		}
	}
</script>

<div class="h-full">
	<Resizable.PaneGroup
		direction="horizontal"
		class="h-full"
		autoSaveId={PROJECT_WORKSPACE_MAIN_LAYOUT_ID}
	>
		<Resizable.Pane
			order={1}
			defaultSize={workspace.navCollapsed.current ? 4 : 15}
			collapsedSize={4}
			collapsible
			minSize={15}
			maxSize={15}
			onCollapse={() => workspace.setNavCollapsed(true)}
			onExpand={() => workspace.setNavCollapsed(false)}
			class="min-w-12.5 transition-all duration-300 ease-in-out"
		>
			<ProjectSidebar collapsed={workspace.navCollapsed.current} />
		</Resizable.Pane>
		<Resizable.Handle withHandle />
		<Resizable.Pane order={2} defaultSize={85} minSize={36}>
			<div class="flex h-full min-w-0 flex-col">
				<TopNavBar
					title={viewTitle}
					projects={projectList}
					selectedProjectId={workspace.selectedProjectId}
					onSelectProject={handleSelectProject}
					onCreateProject={workspace.openProjectCreate}
					{agentHref}
				/>
				<div class="min-h-0 flex-1">
					{#if hasInspector}
						<Resizable.PaneGroup
							direction="horizontal"
							class="h-full"
							autoSaveId={PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID}
						>
							<Resizable.Pane order={1} defaultSize={75} minSize={36}>
								<ProjectWorkspaceViewPanel {children} />
							</Resizable.Pane>
							<Resizable.Handle withHandle />
							<Resizable.Pane
								order={2}
								bind:this={inspectorPane}
								defaultSize={workspace.inspectorCollapsed.current ? 4 : 25}
								collapsedSize={4}
								collapsible
								minSize={20}
								maxSize={34}
								onCollapse={() => workspace.setInspectorCollapsed(true)}
								onExpand={() => workspace.setInspectorCollapsed(false)}
								class="min-w-12.5 transition-all duration-300 ease-in-out"
							>
								{#if workspace.view === 'ingestions'}
									<IngestionInspector
										collapsed={workspace.inspectorCollapsed.current}
										onToggleCollapse={toggleInspector}
									/>
								{:else if workspace.view === 'articles' || workspace.view === 'graph' || workspace.view === 'recommendations'}
									<ArticleInspector
										collapsed={workspace.inspectorCollapsed.current}
										onToggleCollapse={toggleInspector}
									/>
								{/if}
							</Resizable.Pane>
						</Resizable.PaneGroup>
					{:else}
						<ProjectWorkspaceViewPanel {children} />
					{/if}
				</div>
			</div>
		</Resizable.Pane>
	</Resizable.PaneGroup>
</div>
