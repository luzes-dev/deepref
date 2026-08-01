import type { ArticleDto, IngestionDto, ProjectDto } from '$lib/api/generated/models';
import { Context, PersistedState, type Getter } from 'runed';
import {
	DEFAULT_PROJECT_MAX_DEPTH,
	PROJECT_INSPECTOR_COLLAPSED_KEY,
	PROJECT_INSPECTOR_SIZE_KEY,
	PROJECT_NAV_COLLAPSED_KEY,
	PROJECT_NAV_SIZE_KEY
} from './constants';
import type { ProjectWorkspaceCounts, ProjectWorkspaceState, ProjectWorkspaceView } from './types';

export type ArticleSort = 'rank' | 'internal' | 'total' | 'year' | 'title';

type ProjectWorkspaceDataSources = {
	projects: Getter<ProjectDto[]>;
	project: Getter<ProjectDto | undefined>;
	articles: Getter<ArticleDto[]>;
	ingestions: Getter<IngestionDto[]>;
	articlesLoading: Getter<boolean>;
	ingestionsLoading: Getter<boolean>;
	articlesError: Getter<string | undefined>;
	ingestionsError: Getter<string | undefined>;
};

class ProjectWorkspaceContext {
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
	navSize = new PersistedState(PROJECT_NAV_SIZE_KEY, 18, { syncTabs: false });
	inspectorCollapsed = new PersistedState(PROJECT_INSPECTOR_COLLAPSED_KEY, false, {
		syncTabs: false
	});
	inspectorSize = new PersistedState(PROJECT_INSPECTOR_SIZE_KEY, 25, { syncTabs: false });
	projectSelectorOpen = $state(false);
	projectCreateOpen = $state(false);
	projectManagementOpen = $state(false);
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

	#resetSelection = (projectId?: string) => {
		this.workspaceState.project = projectId;
		this.workspaceState.article = undefined;
		this.workspaceState.ingestion = undefined;
		this.workspaceState.view = 'overview';
	};

	#shouldSelectFirstProject = (projects: ProjectDto[], selectedProjectFailed: boolean) => {
		if (!this.workspaceState.project) return true;
		if (!selectedProjectFailed) return false;
		return !projects.some((project) => project.id === this.workspaceState.project);
	};

	syncProjectSelection = (
		projects: ProjectDto[],
		loading: boolean,
		selectedProjectFailed: boolean
	) => {
		if (loading) return;

		if (projects.length === 0) {
			this.#resetSelection();
			return;
		}

		const firstProjectId = projects[0].id;
		if (this.#shouldSelectFirstProject(projects, selectedProjectFailed)) {
			this.#resetSelection(firstProjectId);
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

	setNavSize = (value: number) => {
		this.navSize.current = value;
	};

	setInspectorCollapsed = (value: boolean) => {
		this.inspectorCollapsed.current = value;
	};

	setInspectorSize = (value: number) => {
		this.inspectorSize.current = value;
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

		if (remainingProjects.length === 0) {
			this.workspaceState.project = undefined;
			this.workspaceState.article = undefined;
			this.workspaceState.ingestion = undefined;
			this.workspaceState.view = 'overview';
			this.closeProjectManagement();
			return;
		}

		if (this.selectedProjectId === projectId) {
			const nextProjectId = remainingProjects[0].id;
			this.workspaceState.project = nextProjectId;
			this.workspaceState.view = 'overview';
			this.workspaceState.article = undefined;
			this.workspaceState.ingestion = undefined;
			this.#resetIngestionMaxDepth(nextProjectId);
		}
	};
}

const projectWorkspaceContext = new Context<ProjectWorkspaceContext>('project-workspace');

export function setProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.set(new ProjectWorkspaceContext());
}

export function useProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.get();
}
