import { goto } from '$app/navigation';
import { navigating, page } from '$app/state';
import { resolve } from '$app/paths';
import type { ResolvedPathname } from '$app/types';
import type { IngestionDto, ProjectDto, ReportDto } from '#lib/api/generated/models/index.js';
import { Context, PersistedState, type Getter } from 'runed';
import { SvelteURLSearchParams } from 'svelte/reactivity';
import { PROJECT_INSPECTOR_COLLAPSED_KEY, PROJECT_NAV_COLLAPSED_KEY } from './constants';
import {
	afterProjectDeleted,
	articleView,
	parseArticleSort,
	projectRouteSelection,
	projectToOpen
} from './workspace-navigation';
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

const VIEW_SUFFIXES = [
	['/protocol', 'protocol'],
	['/prisma', 'prisma'],
	['/articles', 'articles'],
	['/graph', 'graph'],
	['/recommendations', 'recommendations'],
	['/discovery/imports', 'ingestions'],
	['/discovery/duplicates', 'duplicates'],
	['/deduplication', 'duplicates'],
	['/screening/title-abstract', 'screening'],
	['/screening/full-text', 'screening']
] as const satisfies ReadonlyArray<readonly [string, ProjectWorkspaceView]>;

function viewForPathname(pathname: string): ProjectWorkspaceView {
	return VIEW_SUFFIXES.find(([suffix]) => pathname.endsWith(suffix))?.[1] ?? 'overview';
}

const VIEW_PATHS: Record<ProjectWorkspaceNavView, (projectId: string) => ResolvedPathname> = {
	overview: (projectId) => resolve('/projects/[projectId]/overview', { projectId }),
	protocol: (projectId) => resolve('/projects/[projectId]/protocol', { projectId }),
	prisma: (projectId) => resolve('/projects/[projectId]/prisma', { projectId }),
	articles: (projectId) => resolve('/projects/[projectId]/articles', { projectId }),
	graph: (projectId) => resolve('/projects/[projectId]/graph', { projectId }),
	recommendations: (projectId) => resolve('/projects/[projectId]/recommendations', { projectId }),
	ingestions: (projectId) => resolve('/projects/[projectId]/discovery/imports', { projectId })
};

function pathnameForView(projectId: string, view: ProjectWorkspaceNavView): ResolvedPathname {
	return VIEW_PATHS[view](projectId);
}

function appendSearch(pathname: string, params: URLSearchParams): ResolvedPathname {
	const search = params.toString();
	return (search ? `${pathname}?${search}` : pathname) as ResolvedPathname;
}

function navigateTo(url: ResolvedPathname, options?: Parameters<typeof goto>[1]): void {
	void goto(url, options);
}

function setSearchParam(name: string, value: string | undefined): void {
	const params = new SvelteURLSearchParams(page.url.searchParams.toString());
	if (value === undefined) params.delete(name);
	else params.set(name, value);
	navigateTo(appendSearch(page.url.pathname, params), {
		replace: true,
		reset: false
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
	selectedAcquisition = $derived.by(() => page.url.searchParams.get('acquisition') ?? undefined);
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
		},
		update(filter: string, minInternal: number): void {
			const params = new SvelteURLSearchParams(page.url.searchParams.toString());
			if (filter) params.set('filter', filter);
			else params.delete('filter');
			if (minInternal > 0) params.set('minInternal', String(minInternal));
			else params.delete('minInternal');
			navigateTo(appendSearch(page.url.pathname, params), {
				replace: true,
				reset: false
			});
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

	/**
	 * The depth the user chose for the selected project, restored from browser storage on load.
	 * Undefined means the project follows the workspace Settings default.
	 */
	get ingestionDepthChoice(): number | undefined {
		return this.#ingestionDraftProjectId === this.selectedProjectId
			? this.ingestionDraft.maxDepth
			: undefined;
	}

	set ingestionDepthChoice(value: number | undefined) {
		this.#ingestionDraftProjectId = this.selectedProjectId;
		this.ingestionDraft.maxDepth = value;
	}

	#navigateToView = (view: ProjectWorkspaceNavView, search?: URLSearchParams) => {
		if (!this.selectedProjectId) return;
		const params = search ?? new SvelteURLSearchParams(page.url.searchParams.toString());
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
		const route = projectRouteSelection(
			page.params.projectId,
			navigating.to?.params?.projectId,
			page.url.pathname
		);
		const projectId = projectToOpen(
			projects,
			loading,
			route.projectId,
			selectedProjectFailed,
			route.resolvingProject
		);
		if (projectId) navigateTo(pathnameForView(projectId, 'overview'));
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
		const params = new SvelteURLSearchParams(page.url.searchParams.toString());
		params.set('report', reportId);
		params.delete('ingestion');
		this.#navigateToView(articleView(this.view), params);
	};

	clearArticle = () => {
		const params = new SvelteURLSearchParams(page.url.searchParams.toString());
		params.delete('report');
		navigateTo(appendSearch(page.url.pathname, params), { reset: false });
	};

	openIngestion = (ingestionId: string) => {
		if (!ingestionId || !this.selectedProjectId) return;
		const params = new SvelteURLSearchParams(page.url.searchParams.toString());
		params.set('ingestion', ingestionId);
		params.delete('acquisition');
		params.delete('report');
		this.#navigateToView('ingestions', params);
	};

	/** Opens a PubMed ID run in the run inspector. */
	openAcquisition = (acquisitionId: string) => {
		if (!acquisitionId || !this.selectedProjectId) return;
		const params = new SvelteURLSearchParams(page.url.searchParams.toString());
		params.set('acquisition', acquisitionId);
		params.delete('ingestion');
		params.delete('report');
		this.#navigateToView('ingestions', params);
	};

	clearIngestion = () => {
		const params = new SvelteURLSearchParams(page.url.searchParams.toString());
		params.delete('ingestion');
		params.delete('acquisition');
		navigateTo(appendSearch(page.url.pathname, params), { reset: false });
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
		this.closeProjectManagement();
		const next = afterProjectDeleted(this.projects, projectId, this.selectedProjectId);
		if (!next) return;
		if (next.to === 'home') {
			navigateTo(resolve('/'));
			return;
		}
		this.#resetIngestionMaxDepth(next.projectId);
		navigateTo(pathnameForView(next.projectId, 'overview'));
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

const projectWorkspaceContext = new Context<ProjectWorkspaceContext>('project-workspace');

// fallow-ignore-next-line private-type-leak -- Scoped to workspace context provider
export function setProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.set(new ProjectWorkspaceContext());
}

// fallow-ignore-next-line private-type-leak -- Scoped to workspace context provider
export function useProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.get();
}
