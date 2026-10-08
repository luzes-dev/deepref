/**
 * Human labels for evidence citations and provenance. Ids and content hashes
 * are never part of a label: they belong in a tooltip (see EvidenceLabel), so a
 * reviewer reads "p. 2 · “Stroke was defined…”" instead of a UUID.
 */

const QUOTE_LIMIT = 90;
const TITLE_LIMIT = 70;

function collapse(text: string | null | undefined): string {
	return (text ?? '').replace(/\s+/g, ' ').trim();
}

function shorten(text: string, limit: number): string {
	if (text.length <= limit) return text;
	const cut = text.slice(0, limit);
	const boundary = cut.lastIndexOf(' ');
	return `${(boundary > limit * 0.6 ? cut.slice(0, boundary) : cut).trimEnd()}…`;
}

/** A passage quote in curly quotes, shortened at a word boundary; null when empty. */
export function quoteLabel(text: string | null | undefined): string | null {
	const flat = collapse(text);
	return flat ? `“${shorten(flat, QUOTE_LIMIT)}”` : null;
}

/** A report title, shortened for a link or chip; null when empty. */
export function titleLabel(title: string | null | undefined): string | null {
	const flat = collapse(title);
	return flat ? shorten(flat, TITLE_LIMIT) : null;
}

/** "p. 2 · “quote…”", else "p. 2 · Report title", else the page alone. */
export function citationLabel(input: {
	page?: number | null;
	quote?: string | null;
	title?: string | null;
	fallback?: string;
}): string {
	const parts = [
		input.page == null ? null : `p. ${input.page}`,
		quoteLabel(input.quote) ?? titleLabel(input.title)
	].filter((part): part is string => Boolean(part));
	if (parts.length) return parts.join(' · ');
	return input.fallback ?? 'Evidence';
}

/** Turns a stored code such as "does_not_meet" into "Does not meet". */
export function humanizeCode(code: string): string {
	const words = code.replace(/[_-]+/g, ' ').trim().toLowerCase();
	return words ? words.charAt(0).toUpperCase() + words.slice(1) : '';
}
