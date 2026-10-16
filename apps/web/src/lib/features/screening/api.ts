import {
	getGetProjectProtocolQueryKey,
	getGetScreeningHistoryQueryKey,
	getGetScreeningQueueQueryKey
} from '#lib/api/generated/review/review.js';

export const screeningKeys = {
	protocol: (projectId: string) => getGetProjectProtocolQueryKey(projectId),
	queue: (projectId: string) => getGetScreeningQueueQueryKey(projectId),
	history: (projectId: string, reportId: string) =>
		getGetScreeningHistoryQueryKey(projectId, reportId)
};
