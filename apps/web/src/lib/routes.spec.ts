import { describe, expect, it } from 'vitest';
import { routeMetaForPathname } from './routes';

describe('routeMetaForPathname', () => {
	it.each([
		[
			'/projects/project-1/overview',
			'Overview · DeepRef',
			'A working summary of the evidence corpus, citations, and ingestion history.'
		],
		[
			'/projects/project-1/articles',
			'Articles · DeepRef',
			'Browse imported evidence records and inspect article-level provenance in a DeepRef workspace.'
		],
		[
			'/projects/project-1/discovery/imports',
			'Imports · DeepRef',
			'Start evidence imports and monitor provider ingestion runs in a DeepRef workspace.'
		]
	])('returns centralized metadata for %s', (pathname, title, description) => {
		expect(routeMetaForPathname(pathname)).toEqual({ title, description });
	});
});
