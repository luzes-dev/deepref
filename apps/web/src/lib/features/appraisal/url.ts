export type AppraisalLocation = {
	reportId?: string;
	definitionId?: string;
	definitionVersion?: number;
};

export function parseAppraisalLocation(search: URLSearchParams): AppraisalLocation {
	const version = Number.parseInt(search.get('definition_version') ?? '', 10);
	return {
		reportId: present(search.get('report')),
		definitionId: present(search.get('definition')),
		definitionVersion: Number.isInteger(version) && version > 0 ? version : undefined
	};
}

export function updateAppraisalLocation(
	search: URLSearchParams,
	patch: Partial<AppraisalLocation>
): URLSearchParams {
	const next = new URLSearchParams(search);
	set(next, 'report', patch.reportId);
	set(next, 'definition', patch.definitionId);
	if (patch.definitionVersion === undefined) {
		return next;
	}
	if (patch.definitionVersion > 0) {
		next.set('definition_version', String(patch.definitionVersion));
	} else {
		next.delete('definition_version');
	}
	return next;
}

function set(search: URLSearchParams, key: string, value: string | undefined): void {
	if (value === undefined) return;
	if (value) search.set(key, value);
	else search.delete(key);
}

function present(value: string | null): string | undefined {
	const trimmed = value?.trim();
	return trimmed ? trimmed : undefined;
}
