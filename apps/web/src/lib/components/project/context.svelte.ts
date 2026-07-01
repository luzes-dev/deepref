import type { ArticleDto, IngestionDto, ProjectDto } from '$lib/api/generated/models';
import { Context, PersistedState, type Getter } from 'runed';
import { DEFAULT_PROJECT_MAX_DEPTH, PROJECT_NAV_COLLAPSED_KEY } from './constants';
import type { ProjectWorkspaceCounts, ProjectWorkspaceState, ProjectWorkspaceView } from './types';

export type ArticleSort = 'rank' | 'internal' | 'total' | 'year' | 'title';

export type ProjectWorkspaceDataSources = {
	projects: Getter<ProjectDto[]>;
	project: Getter<ProjectDto | undefined>;
	articles: Getter<ArticleDto[]>;
	ingestions: Getter<IngestionDto[]>;
	articlesLoading: Getter<boolean>;
	ingestionsLoading: Getter<boolean>;
	articlesError: Getter<string | undefined>;
	ingestionsError: Getter<string | undefined>;
};

export class ProjectWorkspaceContext {
	#dataSources = $state<ProjectWorkspaceDataSources>({
		projects: () => [],
		project: () => undefined,
		articles: () => [],
		ingestions: () => [],
		articlesLoading: () => false,
		ingestionsLoading: () => false,
		articlesError: () => undefined,
		ingestionsError: () => undefined
	});

	workspaceState = $state<ProjectWorkspaceState>({ view: 'overview' });
	projects = $derived.by(() => this.#dataSources.projects());
	project = $derived.by(() => this.#dataSources.project() as ProjectDto);
	articles = $derived.by(() => this.#dataSources.articles());
	ingestions = $derived.by(() => this.#dataSources.ingestions());
	articlesLoading = $derived.by(() => this.#dataSources.articlesLoading());
	ingestionsLoading = $derived.by(() => this.#dataSources.ingestionsLoading());
	articlesError = $derived.by(() => this.#dataSources.articlesError());
	ingestionsError = $derived.by(() => this.#dataSources.ingestionsError());

	selectedProjectId = $derived.by(() => this.workspaceState.project ?? '');
	selectedArticle = $derived.by(() => this.workspaceState.article);
	selectedIngestion = $derived.by(() => this.workspaceState.ingestion);
	view = $derived.by(() => this.workspaceState.view);
	counts = $derived.by<ProjectWorkspaceCounts>(() => ({
		articles: this.articles.length,
		ingestions: this.ingestions.length
	}));

	navCollapsed = new PersistedState(PROJECT_NAV_COLLAPSED_KEY, false, { syncTabs: false });
	projectSelectorOpen = $state(false);
	projectCreateOpen = $state(false);
	articleFilters = $state({
		filter: '',
		minInternal: 0,
		sort: 'rank' as ArticleSort
	});
	graphFilters = $state({
		search: '',
		minInternal: 0
	});
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

	setDataSources = (dataSources: ProjectWorkspaceDataSources) => {
		this.#dataSources = dataSources;
	};

	syncProjectSelection = (
		projects: ProjectDto[],
		loading: boolean,
		selectedProjectFailed: boolean
	) => {
		if (loading) return;

		if (projects.length === 0) {
			this.workspaceState.project = undefined;
			this.workspaceState.article = undefined;
			this.workspaceState.ingestion = undefined;
			this.workspaceState.view = 'overview';
			return;
		}

		if (!this.workspaceState.project) {
			this.workspaceState.project = projects[0].id;
			return;
		}

		if (
			selectedProjectFailed &&
			!projects.some((project) => project.id === this.workspaceState.project)
		) {
			this.workspaceState.project = projects[0].id;
			this.workspaceState.article = undefined;
			this.workspaceState.ingestion = undefined;
			this.workspaceState.view = 'overview';
		}
	};

	#resetIngestionMaxDepth = (projectId: string) => {
		this.#ingestionDraftProjectId = projectId;
		this.ingestionDraft.maxDepth = undefined;
	};

	selectProject = (projectId: string) => {
		if (!projectId) return;
		this.workspaceState.project = projectId;
		this.workspaceState.article = undefined;
		this.workspaceState.ingestion = undefined;
		this.#resetIngestionMaxDepth(projectId);
	};

	selectView = (view: ProjectWorkspaceView) => {
		this.workspaceState.view = view;
		if (view !== 'ingestions') this.workspaceState.ingestion = undefined;
	};

	openArticle = (doiKey: string) => {
		if (!doiKey) return;
		this.workspaceState.article = doiKey;
		if (this.workspaceState.view !== 'articles' && this.workspaceState.view !== 'graph') {
			this.workspaceState.view = 'articles';
		}
	};

	clearArticle = () => {
		this.workspaceState.article = undefined;
	};

	openIngestion = (ingestionId: string) => {
		if (!ingestionId) return;
		this.workspaceState.ingestion = ingestionId;
		this.workspaceState.view = 'ingestions';
	};

	clearIngestion = () => {
		this.workspaceState.ingestion = undefined;
	};

	projectCreated = (projectId: string) => {
		if (!projectId) return;
		this.closeProjectCreate();
		this.workspaceState.project = projectId;
		this.workspaceState.view = 'overview';
		this.workspaceState.article = undefined;
		this.workspaceState.ingestion = undefined;
		this.#resetIngestionMaxDepth(projectId);
	};

	switchToIngestionProject = (projectId: string) => {
		if (!projectId) return;
		this.workspaceState.project = projectId;
		this.workspaceState.view = 'ingestions';
		this.workspaceState.article = undefined;
		this.#resetIngestionMaxDepth(projectId);
	};

	setNavCollapsed = (value: boolean) => {
		this.navCollapsed.current = value;
	};

	openProjectCreate = () => {
		this.projectCreateOpen = true;
	};

	closeProjectCreate = () => {
		this.projectCreateOpen = false;
	};

	selectProjectFromSelector = (projectId: string) => {
		this.projectSelectorOpen = false;
		this.selectProject(projectId);
	};

	openCreateFromSelector = () => {
		this.projectSelectorOpen = false;
		this.openProjectCreate();
	};

	finishProjectCreated = (projectId: string) => {
		this.projectCreated(projectId);
	};
}

const projectWorkspaceContext = new Context<ProjectWorkspaceContext>('project-workspace');

export function setProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.set(new ProjectWorkspaceContext());
}

export function useProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.get();
}
