import {
	getGetProjectProtocolQueryKey,
	getListTitleAbstractScreeningQueueQueryKey
} from '$lib/api/generated/review/review';

export const screeningKeys = {
	all: (projectId: string) => ['screening', projectId] as const,
	protocol: (projectId: string) => getGetProjectProtocolQueryKey(projectId),
	titleAbstractQueue: (projectId: string, status = 'unscreened') =>
		getListTitleAbstractScreeningQueueQueryKey(projectId, { status, limit: 100 })
};
