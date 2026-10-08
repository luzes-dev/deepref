export function displayDedupeTitle(title: string | null | undefined) {
	return title?.trim() || 'Untitled source record';
}

export function formatDedupeYear(year: number | null | undefined) {
	return year === null || year === undefined ? 'Year unknown' : String(year);
}

export function formatDedupeJson(value: Record<string, unknown> | null | undefined) {
	if (!value || Object.keys(value).length === 0) return 'None';
	return Object.entries(value)
		.map(([key, item]) => `${key}: ${String(item)}`)
		.join(' · ');
}

export function formatDedupeScore(score: number | null | undefined) {
	return `${((score ?? 0) * 100).toFixed(0)}%`;
}

/** The counts a deduplication run reports back, as the API returns them. */
export type DedupeRunCounts = {
	processed: number;
	proposals_created: number;
	auto_linked: number;
	auto_accepted?: number;
};

/**
 * Splits a run into the numbers the queue shows. Fuzzy proposals that were
 * merged straight away are not "possible duplicates" any more, and they count
 * as merges, which matches the AI activity feed.
 */
export function dedupeRunTotals(run: DedupeRunCounts) {
	const autoAccepted = run.auto_accepted ?? 0;
	return {
		processed: run.processed,
		possible: Math.max(0, run.proposals_created - autoAccepted),
		merged: run.auto_linked + autoAccepted
	};
}

function plural(count: number, word: string) {
	return `${count} ${word}${count === 1 ? '' : 's'}`;
}

/** One line for the run banner, for example "Checked 3 records · 1 possible duplicate · just now". */
export function formatDedupeRunSummary(run: DedupeRunCounts, when: string) {
	const totals = dedupeRunTotals(run);
	return [
		`Checked ${plural(totals.processed, 'record')}`,
		plural(totals.possible, 'possible duplicate'),
		...(totals.merged > 0 ? [`${totals.merged} merged automatically`] : []),
		when
	].join(' · ');
}
