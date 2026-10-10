import { describe, expect, it } from 'vitest';
import type {
	AppraisalDefinitionDto,
	AppraisalQuestionDto
} from '#lib/api/generated/models/index.js';
import {
	buildAppraisalPayload,
	createInitialFormState,
	definitionRequiresEvidence,
	judgmentIsComplete,
	questionHasRequiredEvidence,
	questionIsAsked,
	reconcileConditionalResponses
} from './form';

const definition: AppraisalDefinitionDto = {
	id: 'generic',
	version: 1,
	name: 'Generic',
	description: 'Test definition',
	applicability: { designs: ['rct'], note: null },
	domains: [
		{
			id: 'domain',
			label: 'Domain',
			description: null,
			questions: [
				{
					id: 'score',
					label: 'Score',
					help: null,
					answer_schema: { kind: 'scale', min: 0, max: 2, labels: {} },
					required: true,
					requires_evidence: true
				}
			],
			judgment: {
				options: [{ value: 'adequate', label: 'Adequate' }],
				allow_custom: true,
				required: true
			}
		}
	],
	overall_judgment: {
		options: [{ value: 'adequate', label: 'Adequate' }],
		allow_custom: true,
		required: true
	}
};

describe('generic appraisal form state', () => {
	it('starts scale responses blank and builds multiple evidence references', () => {
		const state = createInitialFormState();
		state.responses = { score: 2 };
		state.evidence = {
			score: [
				{ documentId: 'doc-1', blockId: 'block-1' },
				{ documentId: 'doc-1', blockId: 'block-2' }
			]
		};
		state.domainJudgments = { domain: 'custom domain judgment' };
		state.overallJudgment = 'custom overall judgment';

		expect(createInitialFormState().responses).toEqual({});
		expect(buildAppraisalPayload(definition, state).evidence).toHaveLength(2);
		expect(buildAppraisalPayload(definition, state).overall_judgment).toBe(
			'custom overall judgment'
		);
	});

	it('only disables for evidence when the active definition requires it', () => {
		const question = definition.domains[0].questions[0];
		expect(definitionRequiresEvidence(definition)).toBe(true);
		expect(questionHasRequiredEvidence(question, {})).toBe(false);
		expect(
			questionHasRequiredEvidence(question, {
				score: [{ documentId: 'doc-1', blockId: 'block-1' }]
			})
		).toBe(true);
		expect(judgmentIsComplete(definition.overall_judgment, ' custom judgment ')).toBe(true);
	});
});

describe('conditional questions', () => {
	const yesNo = [
		{ value: 'yes', label: 'Yes' },
		{ value: 'no', label: 'No' },
		{ value: 'not_applicable', label: 'Not applicable' }
	];
	const chain: AppraisalDefinitionDto = {
		...definition,
		id: 'chain',
		domains: [
			{
				id: 'domain',
				label: 'Domain',
				description: null,
				judgment: {
					options: [{ value: 'low_risk', label: 'Low risk' }],
					allow_custom: false,
					required: true
				},
				questions: [
					{
						id: 'aware',
						label: '2.1 Aware?',
						help: null,
						answer_schema: {
							kind: 'enum',
							options: [
								{ value: 'yes', label: 'Yes' },
								{ value: 'no', label: 'No' }
							]
						},
						required: true,
						requires_evidence: false
					},
					{
						id: 'context',
						label: '2.3 If Y to 2.1: Context?',
						help: null,
						answer_schema: { kind: 'enum', options: yesNo },
						required: true,
						requires_evidence: false,
						applies_when: {
							match: 'all',
							clauses: [{ question: 'aware', answers: ['yes'] }]
						}
					},
					{
						id: 'affected',
						label: '2.4 If Y to 2.3: Affected?',
						help: null,
						answer_schema: { kind: 'enum', options: yesNo },
						required: true,
						requires_evidence: false,
						applies_when: {
							match: 'all',
							clauses: [{ question: 'context', answers: ['yes'] }]
						}
					}
				]
			}
		]
	};

	function questionById(id: string): AppraisalQuestionDto {
		const question = chain.domains
			.flatMap((domain) => domain.questions)
			.find((item) => item.id === id);
		if (!question) throw new Error(`missing question ${id}`);
		return question;
	}

	it('asks a question only when its condition holds', () => {
		const context = questionById('context');
		expect(questionIsAsked(questionById('aware'), {})).toBe(true);
		expect(questionIsAsked(context, { aware: 'yes' })).toBe(true);
		expect(questionIsAsked(context, { aware: 'no' })).toBe(false);
		expect(questionIsAsked(context, {})).toBe(false);
	});

	it('asks when any clause holds for an any-match condition', () => {
		const eitherAware: AppraisalQuestionDto = {
			...questionById('context'),
			applies_when: {
				match: 'any',
				clauses: [
					{ question: 'aware', answers: ['yes'] },
					{ question: 'other', answers: ['yes'] }
				]
			}
		};
		expect(questionIsAsked(eitherAware, { aware: 'no', other: 'yes' })).toBe(true);
		expect(questionIsAsked(eitherAware, { aware: 'no', other: 'no' })).toBe(false);
	});

	it('records a question that is no longer asked as not applicable', () => {
		const answers = { aware: 'no', context: 'yes', affected: 'yes' };
		expect(reconcileConditionalResponses(chain, answers)).toEqual({
			aware: 'no',
			context: 'not_applicable',
			affected: 'not_applicable'
		});
		// The input is left untouched.
		expect(answers).toEqual({ aware: 'no', context: 'yes', affected: 'yes' });
	});

	it('clears a question that is asked again so the reviewer answers it', () => {
		const cleared = reconcileConditionalResponses(chain, {
			aware: 'no',
			context: 'not_applicable',
			affected: 'not_applicable'
		});
		expect(reconcileConditionalResponses(chain, { ...cleared, aware: 'yes' })).toEqual({
			aware: 'yes',
			affected: 'not_applicable'
		});
	});

	it('settles a chain of conditions in one pass', () => {
		expect(
			reconcileConditionalResponses(chain, { aware: 'yes', context: 'yes', affected: 'no' })
		).toEqual({ aware: 'yes', context: 'yes', affected: 'no' });
		expect(
			reconcileConditionalResponses(chain, { aware: 'yes', context: 'no', affected: 'yes' })
		).toEqual({ aware: 'yes', context: 'no', affected: 'not_applicable' });
	});
});
