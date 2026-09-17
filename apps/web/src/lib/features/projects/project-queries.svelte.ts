import { createGetProjectProjection } from '$lib/api/generated/projection/projection';

function activeProjectQueryOptions(projectId: string, enabled: boolean) {
	return { query: { enabled: Boolean(projectId && enabled), staleTime: 0 } };
}

export function activeProjectQuery(projectId: string, enabled: boolean) {
	return activeProjectQueryOptions(projectId, enabled);
}

export function createActiveProjectProjection(projectId: () => string, enabled: () => boolean) {
	return createGetProjectProjection(projectId, () =>
		activeProjectQueryOptions(projectId(), enabled())
	);
}
