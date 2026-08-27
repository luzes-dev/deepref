import { resolve } from '$app/paths';
import type {
	AiAppraisalAnswerValueDto,
	AiAppraisalPrefillEvidenceDto,
	AiAppraisalPrefillProposalPayload,
	AiReviewedProposalPayload,
	AppraisalDefinitionDto,
	AppraisalQuestionDto,
	DocumentBlockDto
} from '$lib/api/generated/models';
import { fullTextUrlString } from '$lib/features/full-text/url';
import { createInitialFormState, type AppraisalFormState, type EvidenceSelection } from './form';

export type AppraisalPrefillPayload = Extract<
	AiReviewedProposalPayload,
	{ kind: 'appraisal_prefill' }
>;

export function appraisalAnswerValue(answer: AiAppraisalAnswerValueDto): string | boolean | number {
	switch (answer.kind) {
		case 'enum':
		case 'boolean':
		case 'scale':
		case 'text':
			return answer.value;
		default: {
			const exhaustive: never = answer;
			return exhaustive;
		}
	}
}

export function mapAppraisalPrefillToFormState(
	prefill: AiAppraisalPrefillProposalPayload
): AppraisalFormState {
	const state = createInitialFormState();
	for (const answer of prefill.answers) {
		state.responses[answer.question_id] = appraisalAnswerValue(answer.answer);
		state.evidence[answer.question_id] = answer.evidence.map((evidence) => ({
			documentId: evidence.document_id,
			blockId: evidence.document_block_id
		}));
	}
	state.domainJudgments = { ...prefill.domain_judgments };
	state.overallJudgment = prefill.overall_judgment;
	return state;
}

function answerValueForQuestion(
	question: AppraisalQuestionDto,
	value: unknown
): AiAppraisalAnswerValueDto | undefined {
	switch (question.answer_schema.kind) {
		case 'enum':
			return typeof value === 'string' &&
				question.answer_schema.options.some((option) => option.value === value)
				? { kind: 'enum', value }
				: undefined;
		case 'boolean':
			return typeof value === 'boolean' ? { kind: 'boolean', value } : undefined;
		case 'scale':
			return typeof value === 'number' &&
				Number.isInteger(value) &&
				value >= question.answer_schema.min &&
				value <= question.answer_schema.max
				? { kind: 'scale', value }
				: undefined;
		case 'text':
			return typeof value === 'string' &&
				value.trim().length > 0 &&
				value.length <= question.answer_schema.max_length
				? { kind: 'text', value }
				: undefined;
		default: {
			const exhaustive: never = question.answer_schema;
			return exhaustive;
		}
	}
}

export function resolveAppraisalEvidence(
	selection: EvidenceSelection,
	originalEvidence: AiAppraisalPrefillEvidenceDto[],
	blocks: DocumentBlockDto[]
): AiAppraisalPrefillEvidenceDto | undefined {
	const original = originalEvidence.find(
		(evidence) =>
			evidence.document_id === selection.documentId &&
			evidence.document_block_id === selection.blockId
	);
	if (original) return original;

	const block = blocks.find(
		(candidate) =>
			candidate.document_id === selection.documentId && candidate.id === selection.blockId
	);
	return block
		? {
				document_id: block.document_id,
				document_block_id: block.id,
				page: block.page_number,
				parser_version: block.parser_version,
				content_hash: block.content_hash
			}
		: undefined;
}

export function serializeAppraisalPrefillReview(
	definition: AppraisalDefinitionDto,
	reportId: string,
	state: AppraisalFormState,
	original: AiAppraisalPrefillProposalPayload,
	blocks: DocumentBlockDto[]
): AppraisalPrefillPayload {
	const answers = definition.domains.flatMap((domain) =>
		domain.questions.map((question) => {
			const answer = answerValueForQuestion(question, state.responses[question.id]);
			if (!answer) {
				throw new Error(`Complete a valid answer for “${question.label}”.`);
			}
			const originalAnswer = original.answers.find(
				(candidate) => candidate.question_id === question.id
			);
			const evidence = (state.evidence[question.id] ?? [])
				.map((selection) =>
					resolveAppraisalEvidence(selection, originalAnswer?.evidence ?? [], blocks)
				)
				.filter(
					(candidate): candidate is AiAppraisalPrefillEvidenceDto =>
						candidate !== undefined
				);
			if (question.requires_evidence && evidence.length === 0) {
				throw new Error(`Add evidence for “${question.label}”.`);
			}
			return {
				question_id: question.id,
				answer,
				rationale: originalAnswer?.rationale ?? 'Reviewed by a human reviewer.',
				evidence
			};
		})
	);

	const domainJudgments = { ...state.domainJudgments };
	const payload = {
		kind: 'appraisal_prefill' as const,
		report_id: reportId,
		definition_id: definition.id,
		definition_version: definition.version,
		answers,
		domain_judgments: domainJudgments,
		overall_judgment: state.overallJudgment
	};
	return payload;
}

export function appraisalEvidenceHref(
	projectId: string,
	reportId: string,
	evidence: Pick<AiAppraisalPrefillEvidenceDto, 'document_block_id' | 'page'>
): string {
	return (
		resolve('/projects/[projectId]/screening/full-text', { projectId }) +
		fullTextUrlString({
			filter: 'all',
			report: reportId,
			page: evidence.page,
			block: evidence.document_block_id
		})
	);
}

export function appraisalEvidenceLabel(evidence: AiAppraisalPrefillEvidenceDto): string {
	return `Document ${evidence.document_id} · block ${evidence.document_block_id} · page ${evidence.page} · ${evidence.parser_version} · hash ${evidence.content_hash}`;
}
