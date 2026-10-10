import type { ProjectWorkspaceView } from './types';

const ARTICLE_SORTS = ['rank', 'internal', 'total', 'year', 'title'] as const;

export function parseArticleSort(value: string | null): (typeof ARTICLE_SORTS)[number] {
	return ARTICLE_SORTS.find((sort) => sort === value) ?? 'rank';
}

/** The project to open when the route names none, or names one that failed to load. */
export function projectToOpen(
	projects: readonly { id: string }[],
	loading: boolean,
	routeProjectId: string | undefined,
	routeProjectFailed: boolean,
	routeIsResolvingProject: boolean = false
): string | undefined {
	if (loading || (routeIsResolvingProject && !routeProjectId)) return undefined;
	if (routeProjectId && !routeProjectFailed) return undefined;
	const fallbackProjectId = projects[0]?.id;
	if (!fallbackProjectId || routeProjectId === fallbackProjectId) return undefined;
	return fallbackProjectId;
}

/** Resolves the project ID while a SvelteKit navigation is in flight. */
export function projectRouteSelection(
	pageProjectId: string | undefined,
	targetProjectId: string | undefined,
	pathname: string
): { projectId: string | undefined; resolvingProject: boolean } {
	return {
		projectId: targetProjectId ?? pageProjectId,
		resolvingProject: Boolean(targetProjectId) || /\/projects\/[^/]+(?:\/|$)/.test(pathname)
	};
}

/**
 * Where to go after a project is deleted: home when none are left, the next project when the
 * open one was deleted, and nowhere otherwise.
 */
export function afterProjectDeleted(
	projects: readonly { id: string }[],
	deletedId: string,
	openProjectId: string
): { to: 'home' } | { to: 'project'; projectId: string } | undefined {
	const remaining = projects.filter((project) => project.id !== deletedId);
	const next = remaining[0];
	if (!next) return { to: 'home' };
	return openProjectId === deletedId ? { to: 'project', projectId: next.id } : undefined;
}

/** Articles open beside the graph and recommendations; from anywhere else they open in Articles. */
export function articleView(view: ProjectWorkspaceView): 'graph' | 'recommendations' | 'articles' {
	return view === 'graph' || view === 'recommendations' ? view : 'articles';
}
