import type { ProjectDto } from '$lib/api/generated/models';

type ProjectSummary = Pick<ProjectDto, 'created_at' | 'article_count'>;

const dateFormat = new Intl.DateTimeFormat('en-US', { dateStyle: 'medium' });

function normalize(value: string): string {
	return value.trim().replace(/\s+/g, ' ').toLocaleLowerCase();
}

/**
 * Keeps the projects whose name or description contains every word of the
 * query, ignoring case. Order is preserved. A blank query returns everything.
 */
export function filterProjects<T extends Pick<ProjectDto, 'name' | 'description'>>(
	projects: readonly T[],
	query: string
): T[] {
	const terms = normalize(query).split(' ').filter(Boolean);
	if (terms.length === 0) return [...projects];
	return projects.filter((project) => {
		const haystack = normalize(`${project.name} ${project.description ?? ''}`);
		return terms.every((term) => haystack.includes(term));
	});
}

/** Projects whose name matches the given name, ignoring case and extra spaces. */
export function projectsNamed<T extends Pick<ProjectDto, 'name'>>(
	projects: readonly T[],
	name: string
): T[] {
	const wanted = normalize(name);
	if (!wanted) return [];
	return projects.filter((project) => normalize(project.name) === wanted);
}

export function formatProjectDate(iso: string): string {
	return dateFormat.format(new Date(iso));
}

export function formatArticleCount(count: number): string {
	return `${count} ${count === 1 ? 'article' : 'articles'}`;
}

/** "Created Oct 3, 2026 · 12 articles" disambiguates projects that share a name. */
export function projectSubtitle(project: ProjectSummary): string {
	return `Created ${formatProjectDate(project.created_at)} · ${formatArticleCount(project.article_count)}`;
}
