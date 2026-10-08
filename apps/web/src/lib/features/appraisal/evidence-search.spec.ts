import { describe, expect, it } from 'vitest';
import type { DocumentBlockDto } from '$lib/api/generated/models';
import { blockSnippet, normalizeSearchText, searchEvidenceBlocks } from './evidence-search';

function block(id: string, pageNumber: number, text: string): DocumentBlockDto {
	return {
		id,
		document_id: `doc-${pageNumber}`,
		page_number: pageNumber,
		text,
		parser_version: 'parser.v1',
		content_hash: 'a'.repeat(64)
	} as DocumentBlockDto;
}

const blocks = [
	block('b1', 1, 'Participants were randomised by computer-generated sequence.'),
	block('b2', 2, 'Allocation was concealed with sealed opaque envelopes.'),
	block('b3', 4, 'Outcome assessors were blinded to the arm.'),
	block('b4', 4, 'Loss to follow-up was 12% in the control arm.')
];

describe('evidence block search', () => {
	it('matches every term, ignoring case and diacritics', () => {
		const { results, total } = searchEvidenceBlocks(blocks, 'RANDOMISED sequence', 8);
		expect(total).toBe(1);
		expect(results.map((item) => item.id)).toEqual(['b1']);
		expect(searchEvidenceBlocks(blocks, 'randomisé', 8).total).toBe(1);
	});

	it('finds blocks by page with p4 or p. 4', () => {
		expect(searchEvidenceBlocks(blocks, 'p4', 8).results.map((item) => item.id)).toEqual([
			'b3',
			'b4'
		]);
		expect(
			searchEvidenceBlocks(blocks, 'P. 4 control', 8).results.map((item) => item.id)
		).toEqual(['b4']);
	});

	it('browses the first blocks for an empty query and caps results', () => {
		const browse = searchEvidenceBlocks(blocks, '  ', 2);
		expect(browse.results.map((item) => item.id)).toEqual(['b1', 'b2']);
		expect(browse.total).toBe(4);
		expect(searchEvidenceBlocks(blocks, 'the', 1).results).toHaveLength(1);
	});

	it('reports no matches without throwing', () => {
		expect(searchEvidenceBlocks(blocks, 'zzz-missing', 8)).toEqual({ results: [], total: 0 });
	});

	it('normalises text and shortens long snippets at a sensible length', () => {
		expect(normalizeSearchText('Ärger  ÉTÉ')).toBe('arger  ete');
		expect(blockSnippet('  one\n two  ', 50)).toBe('one two');
		const long = blockSnippet('word '.repeat(60), 40);
		expect(long.endsWith('…')).toBe(true);
		expect(long.length).toBeLessThanOrEqual(40);
	});
});
