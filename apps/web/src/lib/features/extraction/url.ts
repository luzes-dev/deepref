export type ExtractionLocation = {
	studyId?: string;
};

export function parseExtractionLocation(search: URLSearchParams): ExtractionLocation {
	return { studyId: present(search.get('study')) };
}

export function updateExtractionLocation(
	search: URLSearchParams,
	patch: Partial<ExtractionLocation>
): URLSearchParams {
	const next = new URLSearchParams(search);
	if (patch.studyId === undefined) return next;
	if (patch.studyId) next.set('study', patch.studyId);
	else next.delete('study');
	return next;
}

function present(value: string | null): string | undefined {
	const trimmed = value?.trim();
	return trimmed ? trimmed : undefined;
}
