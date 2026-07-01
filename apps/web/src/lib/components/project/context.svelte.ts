import type { ArticleDto, IngestionDto, ProjectDto } from '$lib/api/generated/models';
import { Context, PersistedState, type Getter } from 'runed';
import { DEFAULT_PROJECT_MAX_DEPTH, PROJECT_NAV_COLLAPSED_KEY } from './constants';
import type { ProjectWorkspaceCounts, ProjectWorkspaceState, ProjectWorkspaceView } from './types';

export type ArticleSort = 'rank' | 'internal' | 'total' | 'year' | 'title';

export type ProjectWorkspaceContextInput = {
	projects: Getter<ProjectDto[]>;
	project: Getter<ProjectDto>;
	workspaceState: Getter<ProjectWorkspaceState>;
	articles: Getter<ArticleDto[]>;
	ingestions: Getter<IngestionDto[]>;
	articlesLoading: Getter<boolean>;
	ingestionsLoading: Getter<boolean>;
	articlesError: Getter<string | undefined>;
	ingestionsError: Getter<string | undefined>;

	selectProject: (projectId: string) => void;
	selectView: (view: ProjectWorkspaceView) => void;
	projectCreated: (projectId: string) => void;
	openArticle: (doiKey: string) => void;
	clearArticle: () => void;
	openIngestion: (ingestionId: string) => void;
	clearIngestion: () => void;
	switchToIngestionProject: (projectId: string) => void;
};

export class ProjectWorkspaceContext {
	projects = $derived.by(() => this.input.projects());
	project = $derived.by(() => this.input.project());
	workspaceState = $derived.by(() => this.input.workspaceState());
	articles = $derived.by(() => this.input.articles());
	ingestions = $derived.by(() => this.input.ingestions());
	articlesLoading = $derived.by(() => this.input.articlesLoading());
	ingestionsLoading = $derived.by(() => this.input.ingestionsLoading());
	articlesError = $derived.by(() => this.input.articlesError());
	ingestionsError = $derived.by(() => this.input.ingestionsError());

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
		minInternal: 0,
		selected: null as ArticleDto | null
	});
	#ingestionDraftProjectId = $state<string | undefined>(undefined);
	ingestionDraft = $state({
		dois: '',
		maxDepth: undefined as number | undefined
	});

	constructor(readonly input: ProjectWorkspaceContextInput) {}

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

	#resetIngestionMaxDepth = (projectId: string) => {
		this.#ingestionDraftProjectId = projectId;
		this.ingestionDraft.maxDepth = undefined;
	};

	selectProject = (projectId: string) => {
		this.#resetIngestionMaxDepth(projectId);
		this.input.selectProject(projectId);
	};

	selectView = (view: ProjectWorkspaceView) => {
		if (view !== 'graph') this.resetGraphSelection();
		this.input.selectView(view);
	};

	openArticle = (doiKey: string) => {
		this.resetGraphSelection();
		this.input.openArticle(doiKey);
	};

	clearArticle = () => {
		this.input.clearArticle();
	};

	openIngestion = (ingestionId: string) => {
		this.input.openIngestion(ingestionId);
	};

	clearIngestion = () => {
		this.input.clearIngestion();
	};

	projectCreated = (projectId: string) => {
		this.#resetIngestionMaxDepth(projectId);
		this.input.projectCreated(projectId);
	};

	switchToIngestionProject = (projectId: string) => {
		this.#resetIngestionMaxDepth(projectId);
		this.input.switchToIngestionProject(projectId);
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
		this.closeProjectCreate();
		this.projectCreated(projectId);
	};

	resetGraphSelection = () => {
		this.graphFilters.selected = null;
	};
}

const projectWorkspaceContext = new Context<ProjectWorkspaceContext>('project-workspace');

export function setProjectWorkspaceContext(
	input: ProjectWorkspaceContextInput
): ProjectWorkspaceContext {
	return projectWorkspaceContext.set(new ProjectWorkspaceContext(input));
}

export function useProjectWorkspaceContext(): ProjectWorkspaceContext {
	return projectWorkspaceContext.get();
}
