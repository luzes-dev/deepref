import type { DocumentBlockDto } from '#lib/api/generated/models/index.js';

/** Lower-cases and strips diacritics so "Randomised" matches "randomised" and "randomisé". */
export function normalizeSearchText(value: string): string {
	return value
		.normalize('NFD')
		.replace(/\p{Diacritic}/gu, '')
		.toLowerCase();
}

const PAGE_TERM = /^p\.?(\d+)$/;

/**
 * Finds parsed blocks for the evidence picker. Every term must appear in the block text, or
 * a term such as "p4" or "p. 4" must name its page. An empty query browses the first blocks.
 */
export function searchEvidenceBlocks(
	blocks: readonly DocumentBlockDto[],
	query: string,
	limit: number
): { results: DocumentBlockDto[]; total: number } {
	const terms = normalizeSearchText(query)
		.replace(/\bp\.\s*(?=\d)/g, 'p')
		.split(/\s+/)
		.filter(Boolean);
	if (terms.length === 0) {
		return { results: blocks.slice(0, limit), total: blocks.length };
	}
	const matches = blocks.filter((block) => {
		const text = normalizeSearchText(block.text);
		return terms.every((term) => {
			const page = PAGE_TERM.exec(term);
			return page ? block.page_number === Number(page[1]) : text.includes(term);
		});
	});
	return { results: matches.slice(0, limit), total: matches.length };
}

/** One readable line of block text for list rows and selected-evidence summaries. */
export function blockSnippet(text: string, maxLength = 140): string {
	const collapsed = text.replace(/\s+/g, ' ').trim();
	return collapsed.length <= maxLength
		? collapsed
		: `${collapsed.slice(0, maxLength - 1).trimEnd()}…`;
}
