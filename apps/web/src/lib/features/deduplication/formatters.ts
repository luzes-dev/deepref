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
