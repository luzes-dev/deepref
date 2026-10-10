import {
	createGetProjectReviewProtocol,
	createListProjectReviewProtocolVersions,
	createPublishProjectReviewProtocol,
	createSaveProjectReviewProtocol,
	getGetProjectReviewProtocolQueryKey,
	getListProjectReviewProtocolVersionsQueryKey,
	getProjectReviewProtocol,
	publishProjectReviewProtocol,
	saveProjectReviewProtocol
} from '#lib/api/generated/review/review.js';
import { ApiError } from '#lib/api/custom-fetch.js';

/** A protocol document. The editor endpoint answers `null` while a project has no protocol yet. */
export type ProtocolDto = NonNullable<Awaited<ReturnType<typeof getProjectReviewProtocol>>['data']>;
export type SaveProtocolRequest = Parameters<typeof saveProjectReviewProtocol>[1];
export type PublishProtocolRequest = Parameters<typeof publishProjectReviewProtocol>[1];

export {
	createGetProjectReviewProtocol,
	createListProjectReviewProtocolVersions,
	getListProjectReviewProtocolVersionsQueryKey,
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
