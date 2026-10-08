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
	routeProjectFailed: boolean
): string | undefined {
	if (loading) return undefined;
	if (routeProjectId && !routeProjectFailed) return undefined;
	return projects[0]?.id;
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
