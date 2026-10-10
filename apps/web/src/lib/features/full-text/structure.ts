import type { DocumentBlockDto, DocumentSectionDto } from '#lib/api/generated/models/index.js';

/**
 * The first evidence block that belongs to a parsed section, in reading order.
 *
 * GROBID lists sections flat, while the native blocks carry nested paths such as
 * "Subjects and Methods > Statistical Analysis", and some headings are merged
 * ("Results General Demographics"). So the match tries the exact path first,
 * then a path under a parent, then a heading that ends with the section title.
 */
export function firstBlockOfSection(
	section: Pick<DocumentSectionDto, 'path' | 'title'>,
	blocks: readonly DocumentBlockDto[]
): DocumentBlockDto | undefined {
	const inReadingOrder = [...blocks].sort((a, b) => a.ordinal - b.ordinal);
	const title = section.title.trim();
	const tiers: ((block: DocumentBlockDto) => boolean)[] = [
		(block) => samePath(block.section_path, section.path),
		(block) => startsWithPath(block.section_path, section.path),
		(block) => endsWithPath(block.section_path, section.path),
		(block) =>
			Boolean(title) &&
			block.section_path.some((part) => part === title || part.endsWith(` ${title}`))
	];
	for (const matches of tiers) {
		const found = inReadingOrder.find(matches);
		if (found) return found;
	}
	return undefined;
}

function samePath(actual: readonly string[], expected: readonly string[]): boolean {
	return actual.length === expected.length && startsWithPath(actual, expected);
}

function startsWithPath(actual: readonly string[], expected: readonly string[]): boolean {
	return (
		expected.length > 0 &&
		actual.length >= expected.length &&
		expected.every((part, index) => actual[index] === part)
	);
}

function endsWithPath(actual: readonly string[], expected: readonly string[]): boolean {
	return (
		expected.length > 0 &&
		actual.length >= expected.length &&
		expected.every((part, index) => actual[actual.length - expected.length + index] === part)
	);
}

/** Resolver link for a stored DOI, or null when the value is not a DOI. */
export function doiHref(doi: string | null | undefined): string | null {
	const value = (doi ?? '').trim().replace(/^(https?:\/\/(dx\.)?doi\.org\/|doi:\s*)/i, '');
	if (!/^10\.\S+$/.test(value)) return null;
	return `https://doi.org/${value.split('/').map(encodeURIComponent).join('/')}`;
}

/** Left padding for an outline entry, by heading depth. */
export function outlineIndentClass(depth: number): string {
	if (depth <= 1) return 'pl-0';
	if (depth === 2) return 'pl-3';
	if (depth === 3) return 'pl-6';
	return 'pl-9';
}
