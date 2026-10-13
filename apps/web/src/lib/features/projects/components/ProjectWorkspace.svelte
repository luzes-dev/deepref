<script lang="ts">
	import type { Snippet } from 'svelte';
	import * as Alert from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import { ApiError } from '#lib/api/custom-fetch.js';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { Spinner } from '@deepref/ui/spinner';
	import { shouldPollIngestion } from '#lib/api/helpers.js';
	import {
		getListProjectReportsQueryKey,
		listProjectReports
	} from '#lib/api/generated/reports/reports.js';
	import { getListAcquisitionsQueryKey, listAcquisitions } from '#lib/api/generated/acquisitions/acquisitions.js';
	import {
		createGetProject,
		getListProjectsQueryKey
	} from '#lib/api/generated/projects/projects.js';
	import { fetchProjectsPage } from '#lib/api/pagination.js';
	import { createInfiniteQuery } from '@tanstack/svelte-query';
	import { IsMobile } from '#lib/hooks/is-mobile.svelte.js';
	import { setProjectWorkspaceContext } from '../context.svelte.js';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ProjectWorkspaceDesktop from './ProjectWorkspaceDesktop.svelte';
	import ProjectWorkspaceEmptyState from './ProjectWorkspaceEmptyState.svelte';
	import ProjectWorkspaceMobile from './ProjectWorkspaceMobile.svelte';

	let { children }: { children?: Snippet } = $props();

	const isMobile = new IsMobile();
	const workspace = setProjectWorkspaceContext();
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
		queryKey: getListAcquisitionsQueryKey(selectedProjectId),
		queryFn: ({ pageParam, signal }) => listAcquisitions(selectedProjectId, { cursor: pageParam || undefined, limit: 50 }, { signal }),
		initialPageParam: '',
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		staleTime: 0,
		enabled: Boolean(selectedProjectId),
		refetchInterval: (query) => {
			return query.state.data?.pages.some((page) =>
				page.data.items.some((run) => shouldPollIngestion(run.status) !== false)
			)
				? 2_000
				: false;
		},
		refetchIntervalInBackground: false,
		refetchOnWindowFocus: 'always'
	}));
	const projectAcquisitions = $derived(
		ingestionsQuery.data?.pages.flatMap((page) => page.data.items) ?? []
	);

	workspace.setDataSources({
		projects: () => projects,
		project: () => selectedProject,
		articles: () => articles,
		acquisitions: () => projectAcquisitions,
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
			projectsQuery.isPending
		);
	});
</script>

<div class="flex h-svh flex-col overflow-hidden bg-background">
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
		{:else if projectQuery.error}
			<div class="p-4">
				<Alert.Root variant="destructive">
					<CircleAlertIcon />
					<Alert.Title>
						{projectQuery.error instanceof ApiError && projectQuery.error.status === 404
							? 'Project not found' : 'Project unavailable'}
					</Alert.Title>
					<Alert.Description>{projectQuery.error.message}</Alert.Description>
					<Button variant="outline" onclick={() => void projectQuery.refetch()}>Retry</Button>
				</Alert.Root>
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
