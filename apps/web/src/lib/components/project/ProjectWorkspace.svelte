<script lang="ts">
	import type { Snippet } from 'svelte';
	import * as Alert from '$lib/components/ui/alert';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Spinner } from '$lib/components/ui/spinner';
	import DependencyBanner from '$lib/components/DependencyBanner.svelte';
	import { shouldPollIngestion } from '$lib/api/helpers';
	import {
		getListProjectReportsQueryKey,
		listProjectReports
	} from '$lib/api/generated/reports/reports';
	import { createGetDependencyStatus } from '$lib/api/generated/health/health';
	import { getListIngestionsQueryKey } from '$lib/api/generated/ingestions/ingestions';
	import {
		createGetProject,
		getListProjectsQueryKey
	} from '$lib/api/generated/projects/projects';
	import { fetchIngestionsPage, fetchProjectsPage } from '$lib/api/pagination';
	import { createInfiniteQuery } from '@tanstack/svelte-query';
	import { IsMobile } from '$lib/hooks/is-mobile.svelte';
	import { setProjectWorkspaceContext } from './context.svelte.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ProjectWorkspaceDesktop from './ProjectWorkspaceDesktop.svelte';
	import ProjectWorkspaceEmptyState from './ProjectWorkspaceEmptyState.svelte';
	import ProjectWorkspaceMobile from './ProjectWorkspaceMobile.svelte';

	let { children }: { children?: Snippet } = $props();

	const isMobile = new IsMobile();
	const workspace = setProjectWorkspaceContext();
	const dependenciesQuery = createGetDependencyStatus(() => ({
		query: {
			refetchInterval: 10_000,
			refetchIntervalInBackground: false,
			refetchOnWindowFocus: 'always',
			staleTime: 5_000
		}
	}));
	const projectsQuery = createInfiniteQuery(() => ({
		queryKey: getListProjectsQueryKey(),
		queryFn: ({ pageParam, signal }) => fetchProjectsPage(pageParam || undefined, signal),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined
	}));
	const projects = $derived(projectsQuery.data?.pages.flatMap((page) => page.data.items) ?? []);
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

	const articlesQuery = createInfiniteQuery(() => ({
		queryKey: getListProjectReportsQueryKey(selectedProjectId),
		queryFn: ({ pageParam, signal }) =>
			listProjectReports(
				selectedProjectId,
				{ cursor: pageParam || undefined, limit: 50 },
				{ signal }
			),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		staleTime: 0,
		enabled: Boolean(selectedProjectId)
	}));
	const articles = $derived(articlesQuery.data?.pages.flatMap((page) => page.data.items) ?? []);

	const ingestionsQuery = createInfiniteQuery(() => ({
		queryKey: getListIngestionsQueryKey(),
		queryFn: ({ pageParam, signal }) => fetchIngestionsPage(pageParam || undefined, signal),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		staleTime: 0,
		refetchInterval: (query) => {
			const selected = selectedProjectId;
			const projectIngestions = query.state.data?.pages
				.flatMap((page) => page.data.items)
				.filter((ingestion) => ingestion.project_id === selected);
			return projectIngestions?.some(
				(ingestion) => shouldPollIngestion(ingestion.status) !== false
			)
				? 2_000
				: false;
		},
		refetchIntervalInBackground: false,
		refetchOnWindowFocus: 'always'
	}));
	const projectIngestions = $derived(
		(ingestionsQuery.data?.pages.flatMap((page) => page.data.items) ?? []).filter(
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
		projectsHasNextPage: () => projectsQuery.hasNextPage,
		articlesHasNextPage: () => articlesQuery.hasNextPage,
		ingestionsHasNextPage: () => ingestionsQuery.hasNextPage,
		projectsLoadingMore: () => projectsQuery.isFetchingNextPage,
		articlesLoadingMore: () => articlesQuery.isFetchingNextPage,
		ingestionsLoadingMore: () => ingestionsQuery.isFetchingNextPage,
		loadMoreProjects: () => void projectsQuery.fetchNextPage(),
		loadMoreArticles: () => void articlesQuery.fetchNextPage(),
		loadMoreIngestions: () => void ingestionsQuery.fetchNextPage(),
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

<div class="flex h-svh flex-col overflow-hidden bg-background">
	<div class="shrink-0 p-2 empty:hidden">
		<DependencyBanner
			status={dependenciesQuery.data?.data}
			error={dependenciesQuery.error}
			isFetching={dependenciesQuery.isFetching}
			onRetry={() => void dependenciesQuery.refetch()}
		/>
	</div>
	<div class="min-h-0 flex-1">
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
			<ProjectWorkspaceMobile {children} />
		{:else}
			<ProjectWorkspaceDesktop {children} />
		{/if}
	</div>
</div>
