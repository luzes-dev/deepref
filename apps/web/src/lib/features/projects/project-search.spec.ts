import { describe, expect, it } from 'vitest';
import type { ProjectDto } from '#lib/api/generated/models/index.js';
import {
	filterProjects,
	formatArticleCount,
	projectSubtitle,
	projectsNamed
} from './project-search';

function project(id: string, name: string, description: string | null = null): ProjectDto {
	return {
		id,
		name,
		description,
		default_max_depth: 2,
		created_at: '2026-10-03T12:00:00Z',
		updated_at: '2026-10-03T12:00:00Z',
		article_count: 0
	};
}

const projects = [
	project('1', 'extraction test', 'Stroke rehab review'),
	project('2', 'manual identifier propagation'),
	project('3', 'extraction test'),
	project('4', 'Ventilator weaning', 'sedation protocols in ICU')
];

describe('filterProjects', () => {
	it('returns every project for a blank query, in the original order', () => {
		expect(filterProjects(projects, '   ').map((item) => item.id)).toEqual([
			'1',
			'2',
			'3',
			'4'
		]);
	});

	it('matches names and descriptions case-insensitively', () => {
		expect(filterProjects(projects, 'EXTRACTION').map((item) => item.id)).toEqual(['1', '3']);
		expect(filterProjects(projects, 'icu').map((item) => item.id)).toEqual(['4']);
	});

	it('requires every word of the query to match', () => {
		expect(filterProjects(projects, 'extraction stroke').map((item) => item.id)).toEqual(['1']);
		expect(filterProjects(projects, 'extraction  identifier')).toEqual([]);
	});

	it('does not match on project ids', () => {
		expect(filterProjects([project('9f2c-id', 'Other')], '9f2c')).toEqual([]);
	});
});

describe('projectsNamed', () => {
	it('finds every project with the same name, ignoring case and spacing', () => {
		expect(projectsNamed(projects, '  Extraction   TEST ').map((item) => item.id)).toEqual([
			'1',
			'3'
		]);
	});

	it('returns nothing for a blank name or a name that is only a prefix', () => {
		expect(projectsNamed(projects, '   ')).toEqual([]);
		expect(projectsNamed(projects, 'extraction')).toEqual([]);
	});
});

describe('project subtitles', () => {
	it('pluralises article counts', () => {
		expect(formatArticleCount(0)).toBe('0 articles');
		expect(formatArticleCount(1)).toBe('1 article');
		expect(formatArticleCount(35)).toBe('35 articles');
	});

	it('shows the created date and the article count', () => {
		expect(projectSubtitle({ created_at: '2026-10-03T12:00:00Z', article_count: 12 })).toBe(
			'Created Oct 3, 2026 · 12 articles'
		);
	});
});
