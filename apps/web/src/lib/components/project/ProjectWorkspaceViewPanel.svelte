<script lang="ts">
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import ProjectArticlesView from './ProjectArticlesView.svelte';
	import ProjectGraphView from './ProjectGraphView.svelte';
	import ProjectIngestionsView from './ProjectIngestionsView.svelte';
	import ProjectOverview from './ProjectOverview.svelte';
	import ProjectRecommendationsView from './ProjectRecommendationsView.svelte';

	const workspace = useProjectWorkspaceContext();
</script>

{#if workspace.view === 'overview'}
	<ProjectOverview />
{:else if workspace.view === 'articles'}
	<ProjectArticlesView />
{:else if workspace.view === 'graph'}
	<ProjectGraphView />
{:else if workspace.view === 'recommendations'}
	<ProjectRecommendationsView />
{:else if workspace.view === 'ingestions'}
	<ProjectIngestionsView />
{/if}
