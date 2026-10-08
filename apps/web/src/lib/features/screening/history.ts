import type { ScreeningHistoryItemDto } from '$lib/api/generated/models';

type HistoryItem = Pick<
	ScreeningHistoryItemDto,
	| 'event_kind'
	| 'stage'
	| 'decision'
	| 'actor_kind'
	| 'created_at'
	| 'previous_title_abstract_status'
	| 'result_title_abstract_status'
	| 'previous_full_text_status'
	| 'result_full_text_status'
>;

const stageNames: Record<string, string> = {
	title_abstract: 'title & abstract',
	full_text: 'full text'
};

function stageName(stage: string) {
	return stageNames[stage] ?? stage.replaceAll('_', ' ');
}

function statusName(value: string) {
	return value.replaceAll('_', ' ');
}

function actorName(kind: string) {
	if (kind === 'user') return 'you';
	if (kind === 'automation') return 'automation';
	return kind === 'system' ? 'system' : kind;
}

function verb(decision: string | null | undefined) {
	if (decision === 'include') return 'Included';
	if (decision === 'exclude') return 'Excluded';
	if (decision === 'maybe') return 'Marked as maybe';
	return 'Decision recorded';
}

export function formatHistoryTime(value: string) {
	return new Intl.DateTimeFormat('en-US', {
		month: 'short',
		day: 'numeric',
		hour: '2-digit',
		minute: '2-digit',
		hour12: false
	}).format(new Date(value));
}

/** Plain-language summary of one screening history event. */
export function describeHistoryItem(item: HistoryItem) {
	const headline =
		item.event_kind === 'undo'
			? `Undid the ${stageName(item.stage)} decision`
			: `${verb(item.decision)} at ${stageName(item.stage)}`;
	const meta = `${formatHistoryTime(item.created_at)} · ${actorName(item.actor_kind)}`;
	// Only the stage the event did not target is worth a second line, and only
	// when the event changed it (for example an include opening full-text review).
	const other =
		item.stage === 'full_text'
			? ([
					'title & abstract',
					item.previous_title_abstract_status,
					item.result_title_abstract_status
				] as const)
			: ([
					'full text',
					item.previous_full_text_status,
					item.result_full_text_status
				] as const);
	const change =
		other[1] === other[2]
			? null
			: `${other[0][0].toUpperCase()}${other[0].slice(1)}: ${statusName(other[1])} → ${statusName(other[2])}`;
	return { headline, meta, change };
}
