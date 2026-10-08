// Most exclusions are PICO mismatches, so those come first (in PICO order),
// then the remaining reasons alphabetically, with the catch-all last.
const relevanceOrder = [
	'wrong_population',
	'wrong_intervention',
	'wrong_comparator_outcome',
	'wrong_outcome',
	'wrong_design'
];

function rank(code: string) {
	if (code === 'other') return Number.MAX_SAFE_INTEGER;
	const index = relevanceOrder.indexOf(code);
	return index === -1 ? relevanceOrder.length : index;
}

export function sortExclusionReasons<T extends { code: string; label: string }>(
	reasons: readonly T[]
): T[] {
	return [...reasons].sort(
		(a, b) => rank(a.code) - rank(b.code) || a.label.localeCompare(b.label)
	);
}
