<script lang="ts">
	import type { Snippet } from 'svelte';
	import * as Resizable from '$lib/components/ui/resizable';
	import type { PaneAPI } from 'paneforge';
	import ArticleInspector from './ArticleInspector.svelte';
	import {
		PROJECT_WORKSPACE_INSPECTOR_LAYOUT_ID,
		PROJECT_WORKSPACE_MAIN_LAYOUT_ID
	} from './constants';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSidebar from './ProjectSidebar.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';

	let { children }: { children?: Snippet } = $props();

	const workspace = useProjectWorkspaceContext();
	const hasInspector = $derived(
		workspace.view === 'articles' ||
			workspace.view === 'graph' ||
			workspace.view === 'recommendations' ||
			workspace.view === 'ingestions'
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
</script>

<div class="h-full">
	<Resizable.PaneGroup
		direction="horizontal"
		class="h-full"
		autoSaveId={PROJECT_WORKSPACE_MAIN_LAYOUT_ID}
	>
		<Resizable.Pane
			order={1}
			defaultSize={workspace.navCollapsed.current ? 5 : 18}
			collapsedSize={4}
			collapsible
			minSize={15}
			maxSize={20}
			onCollapse={() => workspace.setNavCollapsed(true)}
			onExpand={() => workspace.setNavCollapsed(false)}
			class="min-w-12.5 transition-all duration-300 ease-in-out"
		>
			<ProjectSidebar collapsed={workspace.navCollapsed.current} />
		</Resizable.Pane>
		<Resizable.Handle withHandle />
		<Resizable.Pane order={2} defaultSize={82} minSize={36}>
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
		</Resizable.Pane>
	</Resizable.PaneGroup>
</div>
