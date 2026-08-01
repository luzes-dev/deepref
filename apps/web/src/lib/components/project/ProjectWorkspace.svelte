<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { shouldPollIngestion } from '$lib/api/helpers';
	import { createListProjectArticles } from '$lib/api/generated/articles/articles';
	import { createListIngestions } from '$lib/api/generated/ingestions/ingestions';
	import { createGetProject, createListProjects } from '$lib/api/generated/projects/projects';
	import { IsMobile } from '$lib/hooks/is-mobile.svelte';
	import { setProjectWorkspaceContext } from './context.svelte.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ProjectWorkspaceDesktop from './ProjectWorkspaceDesktop.svelte';
	import ProjectWorkspaceEmptyState from './ProjectWorkspaceEmptyState.svelte';
	import ProjectWorkspaceMobile from './ProjectWorkspaceMobile.svelte';

	const isMobile = new IsMobile();
	const workspace = setProjectWorkspaceContext();
	const projectsQuery = createListProjects();
	const projects = $derived(projectsQuery.data?.data ?? []);
	const selectedProjectId = $derived(workspace.selectedProjectId);

	const projectQuery = createGetProject(
		() => selectedProjectId,
		() => ({
			query: { enabled: Boolean(selectedProjectId), staleTime: 0 }
		})
	);
	const selectedProject = $derived(
		projectQuery.data?.data ?? projects.find((project) => project.id === selectedProjectId)
	);

	const articlesQuery = createListProjectArticles(
		() => selectedProjectId,
		() => ({
			query: { staleTime: 0, enabled: Boolean(selectedProjectId) }
		})
	);
	const articles = $derived(articlesQuery.data?.data ?? []);

	const ingestionsQuery = createListIngestions(() => ({
		query: {
			staleTime: 0,
			refetchInterval: (query) => {
				const selected = selectedProjectId;
				const projectIngestions = query.state.data?.data.filter(
					(ingestion) => ingestion.project_id === selected
				);
				return projectIngestions?.some(
					(ingestion) => shouldPollIngestion(ingestion.status) !== false
				)
					? 2_000
					: false;
			},
			refetchIntervalInBackground: false,
			refetchOnWindowFocus: 'always'
		}
	}));
	const projectIngestions = $derived(
		(ingestionsQuery.data?.data ?? []).filter(
			(ingestion) => ingestion.project_id === selectedProjectId
		)
	);

	workspace.setDataSources({
		projects: () => projects,
		project: () => selectedProject,
		articles: () => articles,
		ingestions: () => projectIngestions,
		articlesLoading: () => articlesQuery.isPending,
		ingestionsLoading: () => ingestionsQuery.isPending,
		articlesError: () => articlesQuery.error?.message,
		ingestionsError: () => ingestionsQuery.error?.message
	});

	$effect(() => {
		workspace.syncProjectSelection(
			projects,
			projectsQuery.isPending,
			Boolean(projectQuery.error)
		);
	});
</script>

<div class="h-svh overflow-hidden bg-background">
	{#if projectsQuery.error}
		<div class="p-4">
			<Alert.Root variant="destructive">
				<CircleAlertIcon />
				<Alert.Title>Projects unavailable</Alert.Title>
				<Alert.Description>{projectsQuery.error.message}</Alert.Description>
			</Alert.Root>
		</div>
	{:else if projectsQuery.isPending}
		<div class="flex h-full items-center justify-center gap-3">
			<Spinner />
			<span class="text-sm text-muted-foreground">Loading workspace</span>
		</div>
	{:else if projects.length === 0}
		<ProjectWorkspaceEmptyState />
	{:else if !selectedProject}
		<div class="p-4">
			<Skeleton class="h-80" />
		</div>
	{:else if isMobile.current}
		<ProjectWorkspaceMobile />
	{:else}
		<ProjectWorkspaceDesktop />
	{/if}
</div>
