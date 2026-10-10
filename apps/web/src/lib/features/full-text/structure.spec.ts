import { describe, expect, it } from 'vitest';
import type { DocumentBlockDto } from '#lib/api/generated/models/index.js';
import { doiHref, firstBlockOfSection, outlineIndentClass } from './structure';

function block(overrides: Partial<DocumentBlockDto>): DocumentBlockDto {
	return {
		content_hash: 'hash',
		document_id: 'document-1',
		id: 'block',
		kind: 'text',
		ordinal: 0,
		page_number: 1,
		page_ocr_required: false,
		parser_version: 'parser',
		section_path: [],
		text: 'Passage',
		...overrides
	};
}

function section(path: string[]) {
	return { path, title: path.at(-1) ?? '' };
}

describe('firstBlockOfSection', () => {
	const blocks = [
		block({ id: 'third', ordinal: 3, section_path: ['Methods'] }),
		block({ id: 'first', ordinal: 1, section_path: ['Methods'] }),
		block({ id: 'results', ordinal: 2, section_path: ['Results'] })
	];

	it('picks the earliest block in reading order that carries the exact section path', () => {
		expect(firstBlockOfSection(section(['Methods']), blocks)?.id).toBe('first');
	});

	it('finds a flat outline entry nested under a parent heading in the native blocks', () => {
		const nested = [
			block({
				id: 'nested',
				ordinal: 22,
				section_path: ['Subjects and Methods', 'Statistical Analysis']
			})
		];
		expect(firstBlockOfSection(section(['Statistical Analysis']), nested)?.id).toBe('nested');
	});

	it('finds a section whose heading was merged with the next line', () => {
		const merged = [
			block({
				id: 'merged',
				ordinal: 24,
				section_path: ['Subjects and Methods', 'Results General Demographics']
			})
		];
		expect(firstBlockOfSection(section(['General Demographics']), merged)?.id).toBe('merged');
	});

	it('does not treat a prefix of a merged heading as the section itself', () => {
		const merged = [
			block({
				id: 'merged',
				ordinal: 24,
				section_path: ['Subjects and Methods', 'Results General Demographics']
			})
		];
		expect(firstBlockOfSection(section(['Results']), merged)).toBeUndefined();
	});

	it('returns undefined when no block belongs to the section', () => {
		expect(firstBlockOfSection(section(['Discussion']), blocks)).toBeUndefined();
		expect(firstBlockOfSection({ path: [], title: '' }, blocks)).toBeUndefined();
	});
});

describe('doiHref', () => {
	it('builds a resolver link for a DOI and keeps its slashes', () => {
		expect(doiHref('10.1161/STROKEAHA.107.484196')).toBe(
			'https://doi.org/10.1161/STROKEAHA.107.484196'
		);
	});

	it('accepts a DOI written as a URL or with a doi prefix', () => {
		expect(doiHref('https://doi.org/10.1000/abc')).toBe('https://doi.org/10.1000/abc');
		expect(doiHref('doi: 10.1000/abc')).toBe('https://doi.org/10.1000/abc');
	});

	it('encodes characters that would change the link target', () => {
		expect(doiHref('10.1000/a#b?c')).toBe('https://doi.org/10.1000/a%23b%3Fc');
	});

	it('returns null for values that are not DOIs', () => {
		expect(doiHref(null)).toBeNull();
		expect(doiHref('')).toBeNull();
		expect(doiHref('javascript:alert(1)')).toBeNull();
		expect(doiHref('10.1000/has space')).toBeNull();
	});
});

describe('outlineIndentClass', () => {
	it('indents deeper headings further', () => {
		expect(outlineIndentClass(1)).toBe('pl-0');
		expect(outlineIndentClass(2)).toBe('pl-3');
		expect(outlineIndentClass(5)).toBe('pl-9');
	});
});
