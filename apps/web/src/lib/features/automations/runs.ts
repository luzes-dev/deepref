/**
 * Plain-language labels for the run history.
 */

const TRIGGER_COPY: Record<string, string> = {
	manual: 'Run by hand',
	schedule: 'Schedule',
	webhook: 'Web address call',
	email: 'E-mail',
	publication_alert: 'New publications'
};

export interface RunLabelInput {
	trigger: string;
	test_mode: boolean;
}

/** "Test run" for test runs, otherwise what started the run. */
export function runLabel(run: RunLabelInput): string {
	if (run.test_mode || run.trigger === 'test') return 'Test run';
	return TRIGGER_COPY[run.trigger] ?? 'Started automatically';
}

export interface ReviewCounts {
	included: number;
	excluded: number;
	unsure: number;
}

/**
 * "9 would include · 3 not sure" for a run that used an AI review. Zero counts
 * are left out. `null` when the run used no AI review.
 */
export function reviewCountsLine(counts: ReviewCounts | null | undefined): string | null {
	if (!counts) return null;
	const parts = [
		counts.included > 0 ? `${counts.included} would include` : null,
		counts.excluded > 0 ? `${counts.excluded} would exclude` : null,
		counts.unsure > 0 ? `${counts.unsure} not sure` : null
	].filter((part): part is string => part !== null);
	return parts.length ? parts.join(' · ') : 'The AI review had no records to judge';
}
