<script lang="ts">
	import * as Resizable from '$lib/components/ui/resizable';
	import ArticleInspector from './ArticleInspector.svelte';
	import { PROJECT_WORKSPACE_MAIN_LAYOUT_ID } from './constants';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSidebar from './ProjectSidebar.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';

	const workspace = useProjectWorkspaceContext();
</script>

<div class="h-full">
	<Resizable.PaneGroup
		direction="horizontal"
		class="h-full"
		autoSaveId={PROJECT_WORKSPACE_MAIN_LAYOUT_ID}
	>
		<Resizable.Pane
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
		<Resizable.Pane defaultSize={workspace.view === 'overview' ? 82 : 57} minSize={36}>
			<ProjectWorkspaceViewPanel />
		</Resizable.Pane>
		{#if workspace.view !== 'overview'}
			<Resizable.Handle withHandle />
			<Resizable.Pane defaultSize={25} minSize={20} maxSize={34}>
				{#if workspace.view === 'ingestions'}
					<IngestionInspector />
				{:else}
					<ArticleInspector />
				{/if}
			</Resizable.Pane>
		{/if}
	</Resizable.PaneGroup>
</div>
