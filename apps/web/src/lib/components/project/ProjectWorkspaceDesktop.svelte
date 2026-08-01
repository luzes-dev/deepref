<script lang="ts">
	import * as Resizable from '$lib/components/ui/resizable';
	import type { PaneAPI } from 'paneforge';
	import ArticleInspector from './ArticleInspector.svelte';
	import {
		PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID,
		PROJECT_WORKSPACE_OVERVIEW_LAYOUT_ID
	} from './constants';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSidebar from './ProjectSidebar.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';

	const workspace = useProjectWorkspaceContext();
	const hasInspector = $derived(workspace.view !== 'overview');
	const layoutId = $derived(
		hasInspector ? PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID : PROJECT_WORKSPACE_OVERVIEW_LAYOUT_ID
	);
	let navPane = $state<PaneAPI | undefined>(undefined);
	let inspectorPane = $state<PaneAPI | undefined>(undefined);

	function toggleInspector() {
		if (!inspectorPane) return;
		if (inspectorPane.isCollapsed()) {
			inspectorPane.expand();
			return;
		}

		inspectorPane.collapse();
	}

	$effect(() => {
		hasInspector;

		queueMicrotask(() => {
			if (navPane && !workspace.navCollapsed.current) {
				const targetSize = workspace.navSize.current;
				if (Math.abs(navPane.getSize() - targetSize) > 0.5) {
					navPane.resize(targetSize);
				}
			}

			if (inspectorPane && !workspace.inspectorCollapsed.current) {
				const targetSize = workspace.inspectorSize.current;
				if (Math.abs(inspectorPane.getSize() - targetSize) > 0.5) {
					inspectorPane.resize(targetSize);
				}
			}
		});
	});
</script>

<div class="h-full">
	{@key layoutId}
		<Resizable.PaneGroup
			direction="horizontal"
			class="h-full"
			autoSaveId={layoutId}
		>
			<Resizable.Pane
				order={1}
				bind:this={navPane}
				defaultSize={workspace.navCollapsed.current ? 5 : workspace.navSize.current}
				collapsedSize={4}
				collapsible
				minSize={15}
				maxSize={20}
				onCollapse={() => workspace.setNavCollapsed(true)}
				onExpand={() => workspace.setNavCollapsed(false)}
				onResize={(size) => {
					if (size > 4.5) workspace.setNavSize(size);
				}}
				class="min-w-12.5 transition-all duration-300 ease-in-out"
			>
				<ProjectSidebar collapsed={workspace.navCollapsed.current} />
			</Resizable.Pane>
			<Resizable.Handle withHandle />
			<Resizable.Pane
				order={2}
				defaultSize={hasInspector ? 57 : 82}
				minSize={36}
			>
				<ProjectWorkspaceViewPanel />
			</Resizable.Pane>
			{#if hasInspector}
				<Resizable.Handle withHandle />
				<Resizable.Pane
					order={3}
					bind:this={inspectorPane}
					defaultSize={workspace.inspectorCollapsed.current ? 4 : workspace.inspectorSize.current}
					collapsedSize={4}
					collapsible
					minSize={20}
					maxSize={34}
					onCollapse={() => workspace.setInspectorCollapsed(true)}
					onExpand={() => workspace.setInspectorCollapsed(false)}
					onResize={(size) => {
						if (size > 4.5) workspace.setInspectorSize(size);
					}}
					class="min-w-12.5 transition-all duration-300 ease-in-out"
				>
					{#if workspace.view === 'ingestions'}
						<IngestionInspector
							collapsed={workspace.inspectorCollapsed.current}
							onToggleCollapse={toggleInspector}
						/>
					{:else}
						<ArticleInspector
							collapsed={workspace.inspectorCollapsed.current}
							onToggleCollapse={toggleInspector}
						/>
					{/if}
				</Resizable.Pane>
			{/if}
		</Resizable.PaneGroup>
	{/key}
</div>
