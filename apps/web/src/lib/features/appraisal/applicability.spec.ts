import { describe, expect, it } from 'vitest';
import type { AppraisalDefinitionDto } from '#lib/api/generated/models/index.js';
import { applicabilityStatus, designLabel } from './applicability';

function definition(designs: string[]): AppraisalDefinitionDto {
	return {
		id: 'deepref-rct-rob2',
		version: 1,
		name: 'Randomized trial risk of bias (RoB 2 structure)',
		description: 'RCT tool',
		applicability: { designs, note: null },
		domains: [],
		overall_judgment: { options: [], allow_custom: false, required: true }
	} as AppraisalDefinitionDto;
}

describe('framework applicability', () => {
	it('matches an RCT framework to a trial report', () => {
		expect(
			applicabilityStatus(definition(['rct']), {
				design: 'rct',
				design_label: 'Randomized controlled trial'
			})
		).toEqual({
			kind: 'matches',
			studyLabel: 'Randomized controlled trial'
		});
	});

	it('warns when a cohort study uses an RCT framework, naming what the framework is for', () => {
		expect(
			applicabilityStatus(definition(['rct']), { design: 'cohort', design_label: 'Cohort' })
		).toEqual({
			kind: 'mismatch',
			studyLabel: 'Cohort',
			frameworkLabels: ['randomized controlled trials']
		});
	});

	it('falls back to a readable label when the API sends no label', () => {
		expect(
			applicabilityStatus(definition(['rct']), {
				design: 'cross_sectional',
				design_label: null
			})
		).toMatchObject({
			kind: 'mismatch',
			studyLabel: 'cross-sectional studies'
		});
	});

	it('does not judge a report whose study is unclassified or missing', () => {
		expect(applicabilityStatus(definition(['rct']), undefined)).toEqual({
			kind: 'unclassified'
		});
		expect(applicabilityStatus(definition(['rct']), { design: null })).toEqual({
			kind: 'unclassified'
		});
	});

	it('keeps unknown design values readable rather than hiding them', () => {
		expect(designLabel('new_design')).toBe('new_design');
	});
});
