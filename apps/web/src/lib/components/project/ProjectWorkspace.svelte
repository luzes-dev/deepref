<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import { shouldPollIngestion } from '$lib/api/helpers';
	import { createListProjectArticles } from '$lib/api/generated/articles/articles';
	import { createListIngestions } from '$lib/api/generated/ingestions/ingestions';
	import type { ProjectDto } from '$lib/api/generated/models';
	import { createGetProject, createListProjects } from '$lib/api/generated/projects/projects';
	import { IsMobile } from '$lib/hooks/is-mobile.svelte';
	import { setProjectWorkspaceContext } from './context.svelte.js';
	import type { ProjectWorkspaceView } from './types';
	import {
		buildArticleUrl,
		buildIngestionUrl,
		buildProjectChangeUrl,
		buildProjectWorkspaceUrl,
		parseProjectWorkspaceState
	} from './url';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ProjectWorkspaceDesktop from './ProjectWorkspaceDesktop.svelte';
	import ProjectWorkspaceEmptyState from './ProjectWorkspaceEmptyState.svelte';
	import ProjectWorkspaceMobile from './ProjectWorkspaceMobile.svelte';

	const isMobile = new IsMobile();
	const projectsQuery = createListProjects();
	const workspaceState = $derived(parseProjectWorkspaceState(page.url));
	const projects = $derived(projectsQuery.data?.data ?? []);
	const selectedProjectId = $derived(workspaceState.project ?? '');
	const explicitProjectMissing = $derived(
		Boolean(
			workspaceState.project &&
			!projectsQuery.isPending &&
			!projects.some((project) => project.id === workspaceState.project)
		)
	);

	const projectQuery = createGetProject(
		() => selectedProjectId,
		() => ({
			query: { enabled: Boolean(selectedProjectId && !explicitProjectMissing), staleTime: 0 }
		})
	);
	const selectedProject = $derived(
		projectQuery.data?.data ?? projects.find((project) => project.id === workspaceState.project)
	);

	const articlesQuery = createListProjectArticles(
		() => selectedProjectId,
		() => ({
			query: { staleTime: 0, enabled: Boolean(selectedProjectId && !explicitProjectMissing) }
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

	function replaceUrl(url: string) {
		return goto(resolve(url as '/' | `/?${string}`), {
			replaceState: true,
			noScroll: true
		});
	}

	function pushUrl(url: string) {
		return goto(resolve(url as '/' | `/?${string}`), { noScroll: true });
	}

	function selectProject(projectId: string) {
		void pushUrl(buildProjectChangeUrl(projectId, workspaceState));
	}

	function selectView(view: ProjectWorkspaceView) {
		void pushUrl(buildProjectWorkspaceUrl({ view }, { current: workspaceState }));
	}

	function openArticle(doiKey: string) {
		if (!selectedProjectId || !doiKey) return;
		void pushUrl(buildArticleUrl(selectedProjectId, doiKey));
	}

	function clearArticle() {
		void pushUrl(
			buildProjectWorkspaceUrl(
				{ article: undefined },
				{ current: { ...workspaceState, article: undefined } }
			)
		);
	}

	function openIngestion(ingestionId: string) {
		if (!selectedProjectId || !ingestionId) return;
		void pushUrl(buildIngestionUrl(selectedProjectId, ingestionId));
	}

	function clearIngestion() {
		void pushUrl(
			buildProjectWorkspaceUrl(
				{ ingestion: undefined },
				{ current: { ...workspaceState, ingestion: undefined } }
			)
		);
	}

	function handleProjectCreated(projectId: string) {
		if (!projectId) return;
		void pushUrl(buildProjectWorkspaceUrl({ project: projectId, view: 'overview' }));
	}

	function clearMissingProject() {
		const firstProject = projects[0]?.id;
		void replaceUrl(
			firstProject
				? buildProjectWorkspaceUrl({ project: firstProject, view: 'overview' })
				: buildProjectWorkspaceUrl({ view: 'overview' })
		);
	}

	function switchToIngestionProject(projectId: string) {
		void pushUrl(
			buildProjectWorkspaceUrl({
				project: projectId,
				view: 'ingestions',
				ingestion: workspaceState.ingestion
			})
		);
	}

	setProjectWorkspaceContext({
		projects: () => projects,
		project: () => selectedProject as ProjectDto,
		workspaceState: () => workspaceState,
		articles: () => articles,
		ingestions: () => projectIngestions,
		articlesLoading: () => articlesQuery.isPending,
		ingestionsLoading: () => ingestionsQuery.isPending,
		articlesError: () => articlesQuery.error?.message,
		ingestionsError: () => ingestionsQuery.error?.message,
		selectProject,
		selectView,
		projectCreated: handleProjectCreated,
		openArticle,
		clearArticle,
		openIngestion,
		clearIngestion,
		switchToIngestionProject
	});

	$effect(() => {
		if (projectsQuery.isPending || projects.length === 0 || workspaceState.project) return;
		void replaceUrl(
			buildProjectWorkspaceUrl({ project: projects[0].id, view: workspaceState.view })
		);
	});

	$effect(() => {
		const rawView = page.url.searchParams.get('view');
		if (rawView && rawView !== workspaceState.view) {
			void replaceUrl(
				buildProjectWorkspaceUrl({ ...workspaceState, view: workspaceState.view })
			);
		}
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
	{:else if explicitProjectMissing}
		<div class="flex h-full items-center justify-center p-4">
			<Empty.Root>
				<Empty.Header>
					<Empty.Title>Project not found</Empty.Title>
					<Empty.Description>
						The selected project does not exist or is no longer available.
					</Empty.Description>
				</Empty.Header>
				<Empty.Content>
					<Button onclick={clearMissingProject}>Clear selection</Button>
				</Empty.Content>
			</Empty.Root>
		</div>
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
