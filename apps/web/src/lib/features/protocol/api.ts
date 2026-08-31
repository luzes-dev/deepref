import {
	createGetProjectReviewProtocol,
	createPublishProjectReviewProtocol,
	createSaveProjectReviewProtocol,
	getGetProjectReviewProtocolQueryKey,
	getProjectReviewProtocol,
	publishProjectReviewProtocol,
	saveProjectReviewProtocol
} from '$lib/api/generated/review/review';
import { ApiError } from '$lib/api/custom-fetch';

export type ProtocolDto = Awaited<ReturnType<typeof getProjectReviewProtocol>>['data'];
export type SaveProtocolRequest = Parameters<typeof saveProjectReviewProtocol>[1];
export type PublishProtocolRequest = Parameters<typeof publishProjectReviewProtocol>[1];

export {
	createGetProjectReviewProtocol,
	createPublishProjectReviewProtocol,
	createSaveProjectReviewProtocol,
	getGetProjectReviewProtocolQueryKey
};

export function isNotFound(error: unknown): boolean {
	return error instanceof ApiError && error.status === 404;
}

export function isConflict(error: unknown): boolean {
	return error instanceof ApiError && error.status === 409;
}
