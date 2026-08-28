export type StudyLocation = {
	studyId?: string;
	reportId?: string;
};

export function parseStudyLocation(search: URLSearchParams): StudyLocation {
	return {
		studyId: present(search.get('study')),
		reportId: present(search.get('report'))
	};
}

export function updateStudyLocation(
	search: URLSearchParams,
	patch: Partial<StudyLocation>
): URLSearchParams {
	const next = new URLSearchParams(search);
	for (const [key, value] of [
		['study', patch.studyId],
		['report', patch.reportId]
	] as const) {
		if (value === undefined) continue;
		if (value) next.set(key, value);
		else next.delete(key);
	}
	return next;
}

function present(value: string | null): string | undefined {
	const trimmed = value?.trim();
	return trimmed ? trimmed : undefined;
}
