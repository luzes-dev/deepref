import { describe, expect, it } from 'vitest';
import type {
	AppraisalAssessmentDto,
	AppraisalDefinitionDto
} from '#lib/api/generated/models/index.js';
import { responseLabel } from './renderer';
import { summarizeAssessment } from './summary';

const judgment = {
	options: [{ value: 'low', label: 'Low concern' }],
	allow_custom: true,
	required: true
};
const definition: AppraisalDefinitionDto = {
	id: 'generic',
	version: 1,
	name: 'Generic',
	description: '',
	applicability: { designs: [], note: null },
	domains: [
		{
			id: 'allocation',
			label: 'Allocation',
			description: null,
			judgment,
			questions: [
				{
					id: 'described',
					label: 'Described?',
					help: null,
					answer_schema: {
						kind: 'enum',
						options: [{ value: 'yes', label: 'Yes, fully' }]
					},
					required: true,
					requires_evidence: true
				},
				{
					id: 'blinded',
					label: 'Blinded?',
					help: null,
					answer_schema: { kind: 'boolean' },
					required: true,
					requires_evidence: false
				}
			]
		}
	],
	overall_judgment: judgment
};
const assessment: AppraisalAssessmentDto = {
	id: 'a1',
	project_id: 'p',
	report_id: 'r',
	definition_id: 'generic',
	definition_version: 1,
	responses: { described: 'yes', blinded: false },
	judgments: { domains: { allocation: 'low' }, overall: 'Custom overall' },
	evidence: [{ question_id: 'described', document_id: 'd', block_id: 'b' }],
	actor_kind: 'user',
	actor_id: 'u',
	completed_at: '2026-01-01T00:00:00Z',
	created_at: '2026-01-01T00:00:00Z'
};

describe('assessment summary', () => {
	it('uses framework labels and keeps custom judgments', () => {
		const summary = summarizeAssessment(definition, assessment, responseLabel);
		expect(summary.evidenceCount).toBe(1);
		expect(summary.overall).toBe('Custom overall');
		expect(summary.domains[0]?.judgment).toBe('Low concern');
		expect(summary.domains[0]?.answers.map((answer) => answer.value)).toEqual([
			'Yes, fully',
			'No'
		]);
	});

	it('tolerates empty judgments and responses', () => {
		const summary = summarizeAssessment(
			definition,
			{ ...assessment, responses: {}, judgments: {} },
			responseLabel
		);
		expect(summary.overall).toBeUndefined();
		expect(summary.domains[0]?.answers[0]?.value).toBe('—');
	});
});

describe('rule suggestions and override reasons in the summary', () => {
	const baseAssessment: AppraisalAssessmentDto = {
		id: 'assessment-1',
		project_id: 'project-1',
		report_id: 'report-1',
		definition_id: 'generic',
		definition_version: 1,
		responses: { described: 'yes' },
		judgments: {},
		evidence: [],
		actor_kind: 'user',
		actor_id: 'reviewer',
		completed_at: '2026-10-01T00:00:00Z',
		created_at: '2026-10-01T00:00:00Z'
	};
	const ruleJudgments = {
		options: [
			{ value: 'low_risk', label: 'Low risk' },
			{ value: 'high_risk', label: 'High risk' }
		],
		allow_custom: false,
		required: true
	};
	const ruleDefinition = {
		...definition,
		id: 'deepref-rct-rob2',
		domains: [{ ...definition.domains[0], judgment: ruleJudgments }],
		overall_judgment: ruleJudgments
	} as AppraisalDefinitionDto;

	it('shows what the rules suggested beside the chosen judgment, with the reviewer reason', () => {
		const assessment = {
			...baseAssessment,
			judgments: {
				domains: { allocation: 'low_risk' },
				overall: 'low_risk',
				suggested: {
					available: true,
					domains: [
						{ domain_id: 'allocation', judgment: 'high_risk', drivers: ['described'] }
					],
					overall_judgment: 'high_risk'
				},
				override_reasons: { allocation: 'The protocol shows concealed allocation.' }
			}
		} as unknown as AppraisalAssessmentDto;

		const summary = summarizeAssessment(ruleDefinition, assessment, responseLabel);
		expect(summary.domains[0]).toMatchObject({
			judgment: 'Low risk',
			suggested: 'High risk'
		});
		expect(summary.overallSuggested).toBe('High risk');
		expect(summary.overrides).toEqual([
			{ label: 'Allocation', reason: 'The protocol shows concealed allocation.' }
		]);
	});

	it('has no suggestion or override lines for assessments made without rules', () => {
		const summary = summarizeAssessment(definition, baseAssessment, responseLabel);
		expect(summary.overrides).toEqual([]);
		expect(summary.domains[0].suggested).toBeUndefined();
		expect(summary.overallSuggested).toBeUndefined();
	});
});
