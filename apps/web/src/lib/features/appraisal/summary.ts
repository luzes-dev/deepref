import type {
	AppraisalAssessmentDto,
	AppraisalDefinitionDto
} from '#lib/api/generated/models/index.js';

export type AssessmentSummary = {
	evidenceCount: number;
	overall?: string;
	/** The rule suggestion the reviewer saw, when the framework has one. */
	overallSuggested?: string;
	/** Reviewer reasons for judgments that differed from the rule suggestion. */
	overrides: { label: string; reason: string }[];
	domains: {
		id: string;
		label: string;
		judgment?: string;
		suggested?: string;
		answers: { id: string; label: string; value: string }[];
	}[];
};

function record(value: unknown): Record<string, unknown> {
	return typeof value === 'object' && value !== null ? (value as Record<string, unknown>) : {};
}

function judgmentLabel(
	options: { value: string; label: string }[],
	value: unknown
): string | undefined {
	if (typeof value !== 'string' || value.trim() === '') return undefined;
	return options.find((option) => option.value === value)?.label ?? value;
}

function suggestedFor(suggested: Record<string, unknown>, domainId: string): unknown {
	const domains = Array.isArray(suggested.domains) ? suggested.domains : [];
	return record(domains.find((item) => record(item).domain_id === domainId)).judgment;
}

/** Readable answers and judgments of a completed assessment, using the framework's own labels. */
export function summarizeAssessment(
	definition: AppraisalDefinitionDto,
	assessment: AppraisalAssessmentDto,
	enumLabel: (
		schema: AppraisalDefinitionDto['domains'][number]['questions'][number]['answer_schema'],
		value: unknown
	) => string | undefined
): AssessmentSummary {
	const judgments = record(assessment.judgments);
	// Judgments are stored as { domains, overall }; tolerate a flat domain map.
	const domainJudgments = 'domains' in judgments ? record(judgments.domains) : judgments;
	const suggested = record(judgments.suggested);
	const reasons = record(judgments.override_reasons);
	const labelFor = (key: string): string =>
		key === 'overall'
			? 'Overall judgment'
			: (definition.domains.find((domain) => domain.id === key)?.label ?? key);
	return {
		evidenceCount: assessment.evidence.length,
		overall: judgmentLabel(definition.overall_judgment.options, judgments.overall),
		overallSuggested: judgmentLabel(
			definition.overall_judgment.options,
			suggested.overall_judgment
		),
		overrides: Object.entries(reasons)
			.filter((entry): entry is [string, string] => typeof entry[1] === 'string')
			.map(([key, reason]) => ({ label: labelFor(key), reason })),
		domains: definition.domains.map((domain) => ({
			id: domain.id,
			label: domain.label,
			judgment: judgmentLabel(domain.judgment.options, domainJudgments[domain.id]),
			suggested: judgmentLabel(domain.judgment.options, suggestedFor(suggested, domain.id)),
			answers: domain.questions.map((question) => {
				const value = assessment.responses[question.id];
				const text =
					typeof value === 'boolean'
						? value
							? 'Yes'
							: 'No'
						: (enumLabel(question.answer_schema, value) ??
							(value === undefined || value === null || value === ''
								? '—'
								: String(value)));
				return { id: question.id, label: question.label, value: text };
			})
		}))
	};
}
