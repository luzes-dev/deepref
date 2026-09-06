import { goto } from '$app/navigation';
import { page } from '$app/state';
import { resolve } from '$app/paths';
import type { ResolvedPathname } from '$app/types';
import type { IngestionDto, ProjectDto, ReportDto } from '$lib/api/generated/models';
import { Context, PersistedState, type Getter } from 'runed';
import { SvelteURLSearchParams } from 'svelte/reactivity';
import {
	DEFAULT_PROJECT_MAX_DEPTH,
	PROJECT_INSPECTOR_COLLAPSED_KEY,
	PROJECT_NAV_COLLAPSED_KEY
} from './constants';
import type {
	ProjectWorkspaceCounts,
	ProjectWorkspaceNavView,
	ProjectWorkspaceView
} from './types';

export type ArticleSort = 'rank' | 'internal' | 'total' | 'year' | 'title';
const GRAPH_OVERLAY_FIELDS = ['metrics', 'screening', 'study', 'appraisal', 'provenance'] as const;
export type GraphOverlayField = (typeof GRAPH_OVERLAY_FIELDS)[number];
type GraphColorMode = GraphOverlayField;

type ProjectWorkspaceDataSources = {
	projects: Getter<ProjectDto[]>;
	project: Getter<ProjectDto | undefined>;
	articles: Getter<ReportDto[]>;
	ingestions: Getter<IngestionDto[]>;
	articlesLoading: Getter<boolean>;
	ingestionsLoading: Getter<boolean>;
	projectsHasNextPage: Getter<boolean>;
	articlesHasNextPage: Getter<boolean>;
	ingestionsHasNextPage: Getter<boolean>;
	projectsLoadingMore: Getter<boolean>;
	articlesLoadingMore: Getter<boolean>;
	ingestionsLoadingMore: Getter<boolean>;
	loadMoreProjects: () => void;
	loadMoreArticles: () => void;
	loadMoreIngestions: () => void;
	articlesError: Getter<string | undefined>;
	ingestionsError: Getter<string | undefined>;
};

function viewForPathname(pathname: string): ProjectWorkspaceView {
	if (pathname.endsWith('/protocol')) return 'protocol';
	if (pathname.endsWith('/prisma')) return 'prisma';
	if (pathname.endsWith('/articles')) return 'articles';
	if (pathname.endsWith('/graph')) return 'graph';
	if (pathname.endsWith('/recommendations')) return 'recommendations';
	if (pathname.endsWith('/discovery/imports')) return 'ingestions';
	if (pathname.endsWith('/discovery/duplicates')) return 'duplicates';
	if (pathname.endsWith('/screening/title-abstract')) return 'screening';
	return 'overview';
}

function pathnameForView(projectId: string, view: ProjectWorkspaceNavView): ResolvedPathname {
	switch (view) {
		case 'overview':
			return resolve('/projects/[projectId]/overview', { projectId });
		case 'protocol':
			return resolve('/projects/[projectId]/protocol', { projectId });
		case 'prisma':
			return resolve('/projects/[projectId]/prisma', { projectId });
		case 'articles':
			return resolve('/projects/[projectId]/articles', { projectId });
		case 'graph':
			return resolve('/projects/[projectId]/graph', { projectId });
		case 'recommendations':
			return resolve('/projects/[projectId]/recommendations', { projectId });
		case 'ingestions':
			return resolve('/projects/[projectId]/discovery/imports', { projectId });
		default: {
			const exhaustive: never = view;
			return exhaustive;
		}
	}
}

function appendSearch(pathname: string, params: URLSearchParams): ResolvedPathname {
	const search = params.toString();
	return (search ? `${pathname}?${search}` : pathname) as ResolvedPathname;
}

function navigateTo(url: ResolvedPathname, options?: Parameters<typeof goto>[1]): void {
	void goto(url, options);
}

function setSearchParam(name: string, value: string | undefined): void {
	const params = new SvelteURLSearchParams(page.url.searchParams);
	if (value === undefined) params.delete(name);
	else params.set(name, value);
	navigateTo(appendSearch(page.url.pathname, params), {
		replaceState: true,
		keepFocus: true,
		noScroll: true
	});
}

class ProjectWorkspaceContext {
	#dataSources = $state<ProjectWorkspaceDataSources>({
		projects: () => [],
		project: () => undefined,
		articles: () => [],
		ingestions: () => [],
		articlesLoading: () => false,
		ingestionsLoading: () => false,
		projectsHasNextPage: () => false,
		articlesHasNextPage: () => false,
		ingestionsHasNextPage: () => false,
		projectsLoadingMore: () => false,
		articlesLoadingMore: () => false,
		ingestionsLoadingMore: () => false,
		loadMoreProjects: () => undefined,
		loadMoreArticles: () => undefined,
		loadMoreIngestions: () => undefined,
		articlesError: () => undefined,
		ingestionsError: () => undefined
	});

	projects = $derived.by(() => this.#dataSources.projects());
	project = $derived.by(() => this.#dataSources.project() as ProjectDto);
	articles = $derived.by(() => this.#dataSources.articles());
	ingestions = $derived.by(() => this.#dataSources.ingestions());
	articlesLoading = $derived.by(() => this.#dataSources.articlesLoading());
	ingestionsLoading = $derived.by(() => this.#dataSources.ingestionsLoading());
	projectsHasNextPage = $derived.by(() => this.#dataSources.projectsHasNextPage());
	articlesHasNextPage = $derived.by(() => this.#dataSources.articlesHasNextPage());
	ingestionsHasNextPage = $derived.by(() => this.#dataSources.ingestionsHasNextPage());
	projectsLoadingMore = $derived.by(() => this.#dataSources.projectsLoadingMore());
	articlesLoadingMore = $derived.by(() => this.#dataSources.articlesLoadingMore());
	ingestionsLoadingMore = $derived.by(() => this.#dataSources.ingestionsLoadingMore());
	articlesError = $derived.by(() => this.#dataSources.articlesError());
	ingestionsError = $derived.by(() => this.#dataSources.ingestionsError());

	selectedProjectId = $derived.by(() => page.params.projectId ?? '');
	selectedArticle = $derived.by(() => page.url.searchParams.get('report') ?? undefined);
	selectedIngestion = $derived.by(() => page.url.searchParams.get('ingestion') ?? undefined);
	view = $derived.by(() => viewForPathname(page.url.pathname));
	counts = $derived.by<ProjectWorkspaceCounts>(() => ({
		articles: this.articles.length,
		ingestions: this.ingestions.length
	}));

	navCollapsed = new PersistedState(PROJECT_NAV_COLLAPSED_KEY, false, { syncTabs: false });
	inspectorCollapsed = new PersistedState(PROJECT_INSPECTOR_COLLAPSED_KEY, false, {
		syncTabs: false
	});
	projectSelectorOpen = $state(false);
	projectCreateOpen = $state(false);
	projectManagementOpen = $state(false);

	articleFilters = {
		get filter(): string {
			return page.url.searchParams.get('filter') ?? '';
		},
		set filter(value: string) {
			setSearchParam('filter', value || undefined);
		},
		get minInternal(): number {
			return parseNonNegativeInt(page.url.searchParams.get('minInternal'));
		},
		set minInternal(value: number) {
			setSearchParam('minInternal', value > 0 ? String(value) : undefined);
		},
		get sort(): ArticleSort {
			return parseArticleSort(page.url.searchParams.get('sort'));
		},
		set sort(value: ArticleSort) {
			setSearchParam('sort', value === 'rank' ? undefined : value);
		}
	};

	graphFilters = {
		get search(): string {
			return page.url.searchParams.get('graphSearch') ?? '';
		},
		set search(value: string) {
			setSearchParam('graphSearch', value || undefined);
		},
		get minInternal(): number {
			return parseNonNegativeInt(page.url.searchParams.get('graphMinInternal'));
		},
		set minInternal(value: number) {
			setSearchParam('graphMinInternal', value > 0 ? String(value) : undefined);
		},
		get fields(): GraphOverlayField[] {
			const values = page.url.searchParams.get('graphFields')?.split(',') ?? [];
			const selected = GRAPH_OVERLAY_FIELDS.filter((field) => values.includes(field));
			return selected.length > 0 ? [...selected] : [...GRAPH_OVERLAY_FIELDS];
		},
		setField(field: GraphOverlayField, enabled: boolean) {
			const current = this.fields;
			const selected = GRAPH_OVERLAY_FIELDS.filter((name) =>
				name === field ? enabled : current.includes(name)
			);
			if (selected.length === 0) selected.push('metrics');
			setSearchParam('graphFields', selected.join(','));
		},
		get colorBy(): GraphColorMode {
			const value = page.url.searchParams.get('graphColorBy');
			return GRAPH_OVERLAY_FIELDS.find((field) => field === value) ?? 'metrics';
		},
		set colorBy(value: GraphColorMode) {
			setSearchParam('graphColorBy', value === 'metrics' ? undefined : value);
		}
	};

	#ingestionDraftProjectId = $state<string | undefined>(undefined);
	ingestionDraft = $state({
		dois: '',
		maxDepth: undefined as number | undefined
	});

	get ingestionMaxDepth() {
		const maxDepth =
			this.#ingestionDraftProjectId === this.selectedProjectId
				? this.ingestionDraft.maxDepth
				: undefined;

		return (maxDepth ?? this.project.default_max_depth) || DEFAULT_PROJECT_MAX_DEPTH;
	}

	set ingestionMaxDepth(value: number | undefined) {
		this.#ingestionDraftProjectId = this.selectedProjectId;
		this.ingestionDraft.maxDepth = value;
	}

	#navigateToView = (view: ProjectWorkspaceNavView, search?: URLSearchParams) => {
		if (!this.selectedProjectId) return;
		const params = search ?? new SvelteURLSearchParams(page.url.searchParams);
		if (view !== 'ingestions') params.delete('ingestion');
		navigateTo(appendSearch(pathnameForView(this.selectedProjectId, view), params));
	};

	setDataSources = (dataSources: ProjectWorkspaceDataSources) => {
		this.#dataSources = dataSources;
	};

	loadMoreProjects = () => this.#dataSources.loadMoreProjects();
	loadMoreArticles = () => this.#dataSources.loadMoreArticles();
	loadMoreIngestions = () => this.#dataSources.loadMoreIngestions();

	syncProjectSelection = (
		projects: ProjectDto[],
		loading: boolean,
		selectedProjectFailed: boolean
	) => {
		if (loading || projects.length === 0) return;

		const routeProjectId = page.params.projectId;
		if (routeProjectId && !selectedProjectFailed) return;

		const firstProjectId = projects[0]?.id;
		if (firstProjectId) navigateTo(pathnameForView(firstProjectId, 'overview'));
	};

	selectProject = (projectId: string) => {
		if (!projectId) return;
		this.#resetIngestionMaxDepth(projectId);
		navigateTo(pathnameForView(projectId, 'overview'));
	};

	selectView = (view: ProjectWorkspaceNavView) => {
		this.#navigateToView(view);
	};

	openArticle = (reportId: string) => {
		if (!reportId || !this.selectedProjectId) return;
		const params = new SvelteURLSearchParams(page.url.searchParams);
		params.set('report', reportId);
		params.delete('ingestion');
		this.#navigateToView(this.view === 'graph' ? 'graph' : 'articles', params);
	};

	clearArticle = () => {
		const params = new SvelteURLSearchParams(page.url.searchParams);
		params.delete('report');
		navigateTo(appendSearch(page.url.pathname, params), { keepFocus: true, noScroll: true });
	};

	openIngestion = (ingestionId: string) => {
		if (!ingestionId || !this.selectedProjectId) return;
		const params = new SvelteURLSearchParams(page.url.searchParams);
		params.set('ingestion', ingestionId);
		params.delete('report');
		this.#navigateToView('ingestions', params);
	};

	clearIngestion = () => {
		const params = new SvelteURLSearchParams(page.url.searchParams);
		params.delete('ingestion');
		navigateTo(appendSearch(page.url.pathname, params), { keepFocus: true, noScroll: true });
	};

	projectCreated = (projectId: string) => {
		if (!projectId) return;
		this.closeProjectCreate();
		this.#resetIngestionMaxDepth(projectId);
		navigateTo(pathnameForView(projectId, 'overview'));
	};

	switchToIngestionProject = (projectId: string) => {
		if (!projectId) return;
		this.#resetIngestionMaxDepth(projectId);
		navigateTo(pathnameForView(projectId, 'ingestions'));
	};

	setNavCollapsed = (value: boolean) => {
		this.navCollapsed.current = value;
	};

	setInspectorCollapsed = (value: boolean) => {
		this.inspectorCollapsed.current = value;
	};

	openProjectCreate = () => {
		this.projectCreateOpen = true;
	};

	closeProjectCreate = () => {
		this.projectCreateOpen = false;
	};

	openProjectManagement = () => {
		this.projectManagementOpen = true;
	};

	closeProjectManagement = () => {
		this.projectManagementOpen = false;
	};

	selectProjectFromSelector = (projectId: string) => {
		this.projectSelectorOpen = false;
		this.selectProject(projectId);
	};

	openCreateFromSelector = () => {
		this.projectSelectorOpen = false;
		this.openProjectCreate();
	};

	openManagementFromSelector = () => {
		this.projectSelectorOpen = false;
		this.openProjectManagement();
	};

	finishProjectCreated = (projectId: string) => {
		this.projectCreated(projectId);
	};

	finishProjectDeleted = (projectId: string) => {
		if (!projectId) return;

		const remainingProjects = this.projects.filter((project) => project.id !== projectId);
		this.closeProjectManagement();

		if (remainingProjects.length === 0) {
			navigateTo(resolve('/'));
			return;
		}

		if (this.selectedProjectId === projectId) {
			const nextProjectId = remainingProjects[0]?.id;
			if (nextProjectId) {
				this.#resetIngestionMaxDepth(nextProjectId);
				navigateTo(pathnameForView(nextProjectId, 'overview'));
			}
		}
	};

	#resetIngestionMaxDepth = (projectId: string) => {
		this.#ingestionDraftProjectId = projectId;
		this.ingestionDraft.maxDepth = undefined;
	};
}

function parseNonNegativeInt(value: string | null): number {
	if (!value) return 0;
	const parsed = Number.parseInt(value, 10);
	return Number.isFinite(parsed) && parsed >= 0 ? parsed : 0;
}

function parseArticleSort(value: string | null): ArticleSort {
	if (value === 'internal' || value === 'total' || value === 'year' || value === 'title') {
		return value;
	}
	return 'rank';
}

const projectWorkspaceContext = new Context<ProjectWorkspaceContext>('project-workspace');

export function setProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.set(new ProjectWorkspaceContext());
}

export function useProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.get();
}
