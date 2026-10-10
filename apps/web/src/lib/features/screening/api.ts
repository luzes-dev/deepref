import {
	getGetProjectProtocolQueryKey,
	getGetScreeningHistoryQueryKey
} from '#lib/api/generated/review/review.js';

export const screeningKeys = {
	protocol: (projectId: string) => getGetProjectProtocolQueryKey(projectId),
	queue: (projectId: string) => ['screening-queue', projectId] as const,
	history: (projectId: string, reportId: string) =>
		getGetScreeningHistoryQueryKey(projectId, reportId)
};
