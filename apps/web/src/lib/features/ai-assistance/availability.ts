import type { AiStatusDto } from '#lib/api/generated/models/index.js';

type AiStatusQueryState = {
	data: { data: Pick<AiStatusDto, 'suggestions_available'> } | undefined;
	isError: boolean;
};

/** Fail open when status cannot be loaded, but wait for a successful status before querying. */
export function canRequestAiSuggestions(status: AiStatusQueryState): boolean {
	return status.isError || status.data?.data.suggestions_available === true;
}
