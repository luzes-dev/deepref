import type {
	AppraisalDefinitionDto,
	AppraisalJudgmentSchemaDto,
	DomainJudgmentSuggestionDto,
	JudgmentSuggestionDto
} from '#lib/api/generated/models/index.js';

/** Key used for the overall judgment in override reasons. Matches the API. */
export const OVERALL_TARGET = 'overall';

/** A judgment the reviewer has to confirm against the rule suggestion. */
export type JudgmentTarget = {
	key: string;
	label: string;
	suggested: string | null | undefined;
	chosen: string | undefined;
};

export function suggestionForDomain(
	suggestion: JudgmentSuggestionDto | undefined,
	domainId: string
): DomainJudgmentSuggestionDto | undefined {
	return suggestion?.domains.find((domain) => domain.domain_id === domainId);
}

/** True when the reviewer has chosen a judgment and the rule suggests a different one. */
export function differsFromSuggestion(
	suggested: string | null | undefined,
	chosen: string | undefined
): boolean {
	return Boolean(suggested) && Boolean(chosen) && suggested !== chosen;
}

/** Targets that differ from the suggestion but have no reason yet. */
export function targetsMissingReason(
	targets: readonly JudgmentTarget[],
	reasons: Record<string, string>
): JudgmentTarget[] {
	return targets.filter(
		(target) =>
			differsFromSuggestion(target.suggested, target.chosen) &&
			!(reasons[target.key] ?? '').trim()
	);
}

/** Reasons to send with the appraisal: trimmed, and only where a reason was written. */
export function presentOverrideReasons(reasons: Record<string, string>): Record<string, string> {
	return Object.fromEntries(
		Object.entries(reasons)
			.map(([key, value]) => [key, value.trim()] as const)
			.filter(([, value]) => value.length > 0)
	);
}

export function judgmentLabel(
	schema: AppraisalJudgmentSchemaDto,
	value: string | null | undefined
): string | undefined {
	if (!value) return undefined;
	return schema.options.find((option) => option.value === value)?.label ?? value;
}

/** Shortens a question for a driver line; full wording stays on the question itself. */
export function shortLabel(label: string, maxLength = 90): string {
	const collapsed = label.replace(/\s+/g, ' ').trim();
	if (collapsed.length <= maxLength) return collapsed;
	const cut = collapsed.slice(0, maxLength - 1);
	const atWord = cut.lastIndexOf(' ');
	return `${(atWord > maxLength * 0.6 ? cut.slice(0, atWord) : cut).trimEnd()}…`;
}

/** Question wording for the question ids that drove a suggestion, in the given order. */
export function driverLabels(
	definition: AppraisalDefinitionDto,
	questionIds: readonly string[]
): string[] {
	const labels = new Map(
		definition.domains.flatMap((domain) =>
			domain.questions.map((question) => [question.id, question.label] as const)
		)
	);
	return questionIds.map((id) => shortLabel(labels.get(id) ?? id));
}
