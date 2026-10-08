import type {
	AppraisalConditionClauseDto,
	AppraisalDefinitionDto,
	AppraisalEvidenceRequest,
	AppraisalJudgmentSchemaDto,
	AppraisalQuestionDto,
	CompleteAppraisalRequest,
	CompleteAppraisalRequestDomainJudgments,
	CompleteAppraisalRequestResponses
} from '$lib/api/generated/models';
import { presentOverrideReasons } from './suggestion';

export type EvidenceSelection = {
	documentId: string;
	blockId: string;
};

export type AppraisalFormState = {
	responses: CompleteAppraisalRequestResponses;
	evidence: Record<string, EvidenceSelection[]>;
	domainJudgments: CompleteAppraisalRequestDomainJudgments;
	overallJudgment: string;
	/** Reviewer reasons for judgments that differ from the rule suggestion (domain id or "overall"). */
	overrideReasons: Record<string, string>;
};

export function createInitialFormState(): AppraisalFormState {
	return {
		responses: {},
		evidence: {},
		domainJudgments: {},
		overallJudgment: '',
		overrideReasons: {}
	};
}

export function questionHasRequiredEvidence(
	question: AppraisalQuestionDto,
	evidence: Record<string, EvidenceSelection[]>
): boolean {
	if (!question.requires_evidence) return true;
	return (evidence[question.id] ?? []).some(
		(selection) => selection.documentId.length > 0 && selection.blockId.length > 0
	);
}

export function definitionRequiresEvidence(definition: AppraisalDefinitionDto): boolean {
	return definition.domains.some((domain) =>
		domain.questions.some((question) => question.requires_evidence)
	);
}

/** Answer for a conditional question that the answers so far do not ask. */
export const NOT_APPLICABLE = 'not_applicable';

/** Whether the question is asked for these answers. Questions without a condition are always asked. */
export function questionIsAsked(
	question: AppraisalQuestionDto,
	responses: CompleteAppraisalRequestResponses
): boolean {
	const condition = question.applies_when;
	if (!condition) return true;
	const clauseHolds = (clause: AppraisalConditionClauseDto) => {
		const answer = responses[clause.question];
		return typeof answer === 'string' && clause.answers.includes(answer);
	};
	return condition.match === 'all'
		? condition.clauses.every(clauseHolds)
		: condition.clauses.some(clauseHolds);
}

/**
 * Keeps conditional answers in step with the answers they depend on. A question that is
 * not asked is recorded as not applicable; one that is asked again is cleared, so the
 * reviewer answers it. Questions come in guidance order, so a chain settles in one pass.
 */
export function reconcileConditionalResponses(
	definition: AppraisalDefinitionDto,
	responses: CompleteAppraisalRequestResponses
): CompleteAppraisalRequestResponses {
	const next: CompleteAppraisalRequestResponses = { ...responses };
	for (const question of definition.domains.flatMap((domain) => domain.questions)) {
		if (!question.applies_when) continue;
		if (questionIsAsked(question, next)) {
			if (next[question.id] === NOT_APPLICABLE) delete next[question.id];
		} else {
			next[question.id] = NOT_APPLICABLE;
		}
	}
	return next;
}

export function judgmentIsComplete(
	schema: AppraisalJudgmentSchemaDto,
	value: string | undefined
): boolean {
	return !schema.required || (value !== undefined && value.trim().length > 0);
}

export function buildAppraisalPayload(
	definition: AppraisalDefinitionDto,
	state: AppraisalFormState
): CompleteAppraisalRequest {
	const evidence: AppraisalEvidenceRequest[] = Object.entries(state.evidence).flatMap(
		([questionId, selections]) =>
			selections
				.filter(
					(selection) => selection.documentId.length > 0 && selection.blockId.length > 0
				)
				.map((selection) => ({
					question_id: questionId,
					document_id: selection.documentId,
					block_id: selection.blockId
				}))
	);
	return {
		definition_id: definition.id,
		definition_version: definition.version,
		responses: state.responses,
		domain_judgments: state.domainJudgments,
		overall_judgment: state.overallJudgment || null,
		evidence,
		override_reasons: presentOverrideReasons(state.overrideReasons)
	};
}
