import { describe, expect, it } from 'vitest';
import {
	afterProjectDeleted,
	articleView,
	parseArticleSort,
	projectRouteSelection,
	projectToOpen
} from './workspace-navigation';

const projects = [{ id: 'p1' }, { id: 'p2' }];

describe('workspace navigation', () => {
	it('parses the article sort and falls back to rank', () => {
		expect(parseArticleSort('year')).toBe('year');
		expect(parseArticleSort('title')).toBe('title');
		expect(parseArticleSort('nonsense')).toBe('rank');
		expect(parseArticleSort(null)).toBe('rank');
	});

	it('opens the first project only when the route has no usable project', () => {
		expect(projectToOpen(projects, true, undefined, false)).toBeUndefined();
		expect(projectToOpen([], false, undefined, false)).toBeUndefined();
		expect(projectToOpen(projects, false, 'p2', false)).toBeUndefined();
		expect(projectToOpen(projects, false, 'p2', true)).toBe('p1');
		expect(projectToOpen(projects, false, 'p1', true)).toBeUndefined();
		expect(projectToOpen(projects, false, undefined, false)).toBe('p1');
		expect(projectToOpen(projects, false, undefined, false, true)).toBeUndefined();
	});

	it('keeps the target project while a project navigation is pending', () => {
		expect(projectRouteSelection(undefined, 'p2', '/')).toEqual({
			projectId: 'p2',
			resolvingProject: true
		});
		expect(projectRouteSelection('p1', undefined, '/projects/p1/overview')).toEqual({
			projectId: 'p1',
			resolvingProject: true
		});
		expect(projectRouteSelection(undefined, undefined, '/')).toEqual({
			projectId: undefined,
			resolvingProject: false
		});
	});

	it('decides where to go after a project is deleted', () => {
		expect(afterProjectDeleted([{ id: 'p1' }], 'p1', 'p1')).toEqual({ to: 'home' });
		expect(afterProjectDeleted(projects, 'p1', 'p1')).toEqual({
			to: 'project',
			projectId: 'p2'
		});
		expect(afterProjectDeleted(projects, 'p2', 'p1')).toBeUndefined();
	});

	it('keeps articles beside the graph and recommendations', () => {
		expect(articleView('graph')).toBe('graph');
		expect(articleView('recommendations')).toBe('recommendations');
		expect(articleView('prisma')).toBe('articles');
	});
});
