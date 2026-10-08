export type AutonomyLevel = 'off' | 'suggest' | 'second_reviewer' | 'act';

export const LEVEL_ORDER: readonly AutonomyLevel[] = ['off', 'suggest', 'second_reviewer', 'act'];

export const LEVEL_LABEL: Record<AutonomyLevel, string> = {
	off: 'Off',
	suggest: 'Suggest',
	second_reviewer: 'Second reviewer',
	act: 'Act and notify'
};

export const LEVEL_EXPLANATION: Record<AutonomyLevel, string> = {
	off: 'The AI does nothing for this.',
	suggest: 'The AI prepares suggestions. Nothing changes until you accept one.',
	second_reviewer:
		'The AI decides on its own, kept separate from you. Where you disagree, the record goes to a Conflicts list for you to settle.',
	act: 'The AI does it for you and tells you in the activity feed. You can undo it at any time.'
};

/**
 * "Similar records" matches at or above this score merge on their own when the
 * level is "Act and notify". Keep in step with the `p.score>=0.95` filter in
 * `auto_accept_fuzzy_proposals` (crates/postgres/src/deduplication.rs). Matches
 * below it stay suggestions, whatever the level.
 */
export const FUZZY_AUTO_MERGE_SCORE = 0.95;

export function isAutonomyLevel(value: string): value is AutonomyLevel {
	return (LEVEL_ORDER as readonly string[]).includes(value);
}

export function formatUsd(value: number): string {
	return new Intl.NumberFormat('en-US', {
		style: 'currency',
		currency: 'USD',
		minimumFractionDigits: 2,
		maximumFractionDigits: 2
	}).format(value);
}
