import { describe, expect, it } from 'vitest';
import {
	buildArticleUrl,
	buildIngestionUrl,
	buildProjectChangeUrl,
	buildProjectWorkspaceUrl,
	parseProjectWorkspaceState
} from './url';

describe('project workspace URLs', () => {
	it('parses default URL state', () => {
		expect(parseProjectWorkspaceState(new URL('http://app.test/'))).toEqual({
			project: undefined,
			view: 'overview',
			article: undefined,
			ingestion: undefined
		});
	});

	it('builds project/view/article/ingestion URLs', () => {
		expect(buildProjectWorkspaceUrl({ project: 'p1', view: 'overview' })).toBe(
			'/?project=p1&view=overview'
		);
		expect(buildArticleUrl('p1', 'doi-key')).toBe('/?project=p1&view=articles&article=doi-key');
		expect(buildIngestionUrl('p1', 'ing-1')).toBe(
			'/?project=p1&view=ingestions&ingestion=ing-1'
		);
	});

	it('clears incompatible selection params when project changes', () => {
		expect(
			buildProjectChangeUrl('p2', {
				project: 'p1',
				view: 'articles',
				article: 'doi-key',
				ingestion: 'ing-1'
			})
		).toBe('/?project=p2&view=articles');
	});

	it('normalizes invalid views to overview', () => {
		expect(
			parseProjectWorkspaceState(new URL('http://app.test/?project=p1&view=nope'))
		).toEqual({
			project: 'p1',
			view: 'overview',
			article: undefined,
			ingestion: undefined
		});
		expect(buildProjectWorkspaceUrl({ project: 'p1', view: 'nope' as never })).toBe(
			'/?project=p1&view=overview'
		);
	});
});
