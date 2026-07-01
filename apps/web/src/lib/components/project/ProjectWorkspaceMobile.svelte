<script lang="ts">
	import * as Sheet from '$lib/components/ui/sheet';
	import * as Tabs from '$lib/components/ui/tabs';
	import ArticleInspector from './ArticleInspector.svelte';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import IngestionInspector from './IngestionInspector.svelte';
	import ProjectSelector from './ProjectSelector.svelte';
	import ProjectWorkspaceViewPanel from './ProjectWorkspaceViewPanel.svelte';
	import type { ProjectWorkspaceView } from './types';

	const workspace = useProjectWorkspaceContext();
	const mobileViewLabel = $derived(workspace.view[0].toUpperCase() + workspace.view.slice(1));
	const articleSheetOpen = $derived(Boolean(workspace.selectedArticle));
	const ingestionSheetOpen = $derived(Boolean(workspace.selectedIngestion));
</script>

<div class="flex h-full flex-col">
	<div class="flex flex-col gap-3 border-b p-3">
		<ProjectSelector />
		<Tabs.Root
			value={workspace.view}
			onValueChange={(value) => workspace.selectView(value as ProjectWorkspaceView)}
		>
			<Tabs.List class="w-full overflow-x-auto">
				<Tabs.Trigger value="overview">Overview</Tabs.Trigger>
				<Tabs.Trigger value="articles">Articles</Tabs.Trigger>
				<Tabs.Trigger value="graph">Graph</Tabs.Trigger>
				<Tabs.Trigger value="recommendations">Recs</Tabs.Trigger>
				<Tabs.Trigger value="ingestions">Ingestions</Tabs.Trigger>
			</Tabs.List>
		</Tabs.Root>
		<div class="sr-only">{mobileViewLabel}</div>
	</div>
	<div class="min-h-0 flex-1 overflow-auto">
		<ProjectWorkspaceViewPanel />
	</div>

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
