import { describe, expect, it } from 'vitest';
import type {
	AiAppraisalPrefillProposalPayload,
	AppraisalDefinitionDto,
	DocumentBlockDto
} from '$lib/api/generated/models';
import {
	appraisalAnswerValue,
	appraisalEvidenceHref,
	mapAppraisalPrefillToFormState,
	serializeAppraisalPrefillReview
} from './ai-prefill';

const hash = 'a'.repeat(64);
const definition = {
	id: 'definition-1',
	version: 3,
	name: 'Quality appraisal',
	description: 'Test definition',
	applicability: { designs: ['rct'], note: null },
	domains: [
		{
			id: 'domain-1',
			label: 'Domain',
			description: null,
			judgment: {
				options: [{ value: 'low', label: 'Low concern' }],
				allow_custom: false,
				required: true
			},
			questions: [
				{
					id: 'answer-1',
					label: 'Was it reported?',
					help: null,
					answer_schema: { kind: 'enum', options: [{ value: 'yes', label: 'Yes' }] },
					required: true,
					requires_evidence: true
				},
				{
					id: 'answer-2',
					label: 'Was it prespecified?',
					help: null,
					answer_schema: { kind: 'boolean' },
					required: true,
					requires_evidence: false
				}
			]
		}
	],
	overall_judgment: {
		options: [{ value: 'low', label: 'Low concern' }],
		allow_custom: false,
		required: true
	}
} satisfies AppraisalDefinitionDto;

const block = {
	id: 'block-1',
	document_id: 'document-1',
	page_number: 4,
	ordinal: 0,
	kind: 'text',
	text: 'The allocation was reported.',
	section_path: ['Methods'],
	content_hash: hash,
	parser_version: 'parser-2',
	page_ocr_required: false
} satisfies DocumentBlockDto;

const original = {
	report_id: 'report-1',
	definition_id: definition.id,
	definition_version: definition.version,
	answers: [
		{
			question_id: 'answer-1',
			answer: { kind: 'enum', value: 'yes' },
			rationale: 'The methods section states this clearly.',
			evidence: [
				{
					document_id: block.document_id,
					document_block_id: block.id,
					page: block.page_number,
					parser_version: block.parser_version,
					content_hash: block.content_hash
				}
			]
		},
		{
			question_id: 'answer-2',
			answer: { kind: 'boolean', value: true },
			rationale: 'The prespecified outcome is described.',
			evidence: []
		}
	],
	domain_judgments: { 'domain-1': 'low' },
	overall_judgment: 'low'
} satisfies AiAppraisalPrefillProposalPayload;

describe('AI appraisal prefill transforms', () => {
	it('maps every generated answer variant into the generic form state', () => {
		expect(appraisalAnswerValue({ kind: 'enum', value: 'yes' })).toBe('yes');
		expect(appraisalAnswerValue({ kind: 'boolean', value: true })).toBe(true);
		expect(appraisalAnswerValue({ kind: 'scale', value: 2 })).toBe(2);
		expect(appraisalAnswerValue({ kind: 'text', value: 'note' })).toBe('note');

		const state = mapAppraisalPrefillToFormState(original);
		expect(state.responses).toEqual({ 'answer-1': 'yes', 'answer-2': true });
		expect(state.evidence['answer-1']).toEqual([
			{ documentId: 'document-1', blockId: 'block-1' }
		]);
		expect(state.domainJudgments).toEqual({ 'domain-1': 'low' });
		expect(state.overallJudgment).toBe('low');
	});

	it('serializes reviewer edits with typed answers and exact evidence provenance', () => {
		const state = mapAppraisalPrefillToFormState(original);
		state.responses['answer-1'] = 'yes';
		state.responses['answer-2'] = false;
		state.domainJudgments['domain-1'] = 'low';
		state.overallJudgment = 'low';

		const payload = serializeAppraisalPrefillReview(definition, 'report-1', state, original, [
			block
		]);

		expect(payload).toEqual({
			kind: 'appraisal_prefill',
			report_id: 'report-1',
			definition_id: 'definition-1',
			definition_version: 3,
			answers: [
				{
					question_id: 'answer-1',
					answer: { kind: 'enum', value: 'yes' },
					rationale: 'The methods section states this clearly.',
					evidence: [
						{
							document_id: 'document-1',
							document_block_id: 'block-1',
							page: 4,
							parser_version: 'parser-2',
							content_hash: hash
						}
					]
				},
				{
					question_id: 'answer-2',
					answer: { kind: 'boolean', value: false },
					rationale: 'The prespecified outcome is described.',
					evidence: []
				}
			],
			domain_judgments: { 'domain-1': 'low' },
			overall_judgment: 'low'
		});
	});

	it('builds full-text links with report, page, and block query state', () => {
		expect(
			appraisalEvidenceHref('project-1', 'report-1', {
				document_block_id: 'block-1',
				page: 4
			})
		).toBe('/projects/project-1/screening/full-text?report=report-1&page=4&block=block-1');
	});
});
