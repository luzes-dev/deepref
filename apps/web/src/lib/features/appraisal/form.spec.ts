import { describe, expect, it } from 'vitest';
import type { AppraisalDefinitionDto } from '$lib/api/generated/models';
import {
	buildAppraisalPayload,
	createInitialFormState,
	definitionRequiresEvidence,
	judgmentIsComplete,
	questionHasRequiredEvidence
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
