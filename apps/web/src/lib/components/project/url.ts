import { resolve } from '$app/paths';
import {
	ARTICLE_PARAM,
	INGESTION_PARAM,
	PROJECT_PARAM,
	PROJECTS_ROUTE,
	PROJECT_WORKSPACE_VIEWS,
	VIEW_PARAM,
	type ProjectWorkspaceState,
	type ProjectWorkspaceView
} from './types';

export function normalizeProjectView(value: string | null): ProjectWorkspaceView {
	return PROJECT_WORKSPACE_VIEWS.includes(value as ProjectWorkspaceView)
		? (value as ProjectWorkspaceView)
		: 'overview';
}

export function parseProjectWorkspaceState(url: URL): ProjectWorkspaceState {
	const project = url.searchParams.get(PROJECT_PARAM) || undefined;
	const view = normalizeProjectView(url.searchParams.get(VIEW_PARAM));
	const article = url.searchParams.get(ARTICLE_PARAM) || undefined;
	const ingestion = url.searchParams.get(INGESTION_PARAM) || undefined;

	return {
		project,
		view,
		article: view === 'articles' ? article : undefined,
		ingestion: view === 'ingestions' ? ingestion : undefined
	};
}

export function buildProjectWorkspaceUrl(
	state: Partial<ProjectWorkspaceState>,
	options: { current?: ProjectWorkspaceState; changedProject?: boolean } = {}
): string {
	const next: ProjectWorkspaceState = {
		view: 'overview',
		...options.current,
		...state
	};
	const params = new URLSearchParams();

	if (next.project) params.set(PROJECT_PARAM, next.project);
	params.set(VIEW_PARAM, normalizeProjectView(next.view));

	if (!options.changedProject && next.view === 'articles' && next.article) {
		params.set(ARTICLE_PARAM, next.article);
	}
	if (!options.changedProject && next.view === 'ingestions' && next.ingestion) {
		params.set(INGESTION_PARAM, next.ingestion);
	}

	return `${resolve(PROJECTS_ROUTE)}?${params.toString()}`;
}

export function buildProjectChangeUrl(project: string, current?: ProjectWorkspaceState): string {
	return buildProjectWorkspaceUrl({ project }, { current, changedProject: true });
}

export function buildArticleUrl(project: string, doiKey: string): string {
	return buildProjectWorkspaceUrl({ project, view: 'articles', article: doiKey });
}

export function buildIngestionUrl(project: string, ingestionId: string): string {
	return buildProjectWorkspaceUrl({ project, view: 'ingestions', ingestion: ingestionId });
}
