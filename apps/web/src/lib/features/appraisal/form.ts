import type {
	AppraisalDefinitionDto,
	AppraisalEvidenceRequest,
	AppraisalJudgmentSchemaDto,
	AppraisalQuestionDto,
	CompleteAppraisalRequest,
	CompleteAppraisalRequestDomainJudgments,
	CompleteAppraisalRequestResponses
} from '$lib/api/generated/models';

export type EvidenceSelection = {
	documentId: string;
	blockId: string;
};

export type AppraisalFormState = {
	responses: CompleteAppraisalRequestResponses;
	evidence: Record<string, EvidenceSelection[]>;
	domainJudgments: CompleteAppraisalRequestDomainJudgments;
	overallJudgment: string;
};

export function createInitialFormState(): AppraisalFormState {
	return {
		responses: {},
		evidence: {},
		domainJudgments: {},
		overallJudgment: ''
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
		evidence
	};
}
