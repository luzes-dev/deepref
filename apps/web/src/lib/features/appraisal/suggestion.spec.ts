import { describe, expect, it } from 'vitest';
import type { AppraisalDefinitionDto, JudgmentSuggestionDto } from '$lib/api/generated/models';
import {
	differsFromSuggestion,
	driverLabels,
	judgmentLabel,
	presentOverrideReasons,
	shortLabel,
	suggestionForDomain,
	targetsMissingReason,
	type JudgmentTarget
} from './suggestion';

const definition = {
	id: 'deepref-rct-rob2',
	version: 1,
	domains: [
		{
			id: 'randomization_process',
			questions: [
				{ id: 'sequence_unpredictable', label: 'Was the allocation sequence chance-based?' }
			]
		}
	]
} as unknown as AppraisalDefinitionDto;

const suggestion: JudgmentSuggestionDto = {
	available: true,
	domains: [
		{
			domain_id: 'randomization_process',
			judgment: 'high_risk',
			drivers: ['sequence_unpredictable']
		},
		{ domain_id: 'reported_result', judgment: null, drivers: [] }
	],
	overall_judgment: 'high_risk',
	reviewer_notes: []
};

describe('judgment suggestions in the form', () => {
	it('finds a domain suggestion by id', () => {
		expect(suggestionForDomain(suggestion, 'randomization_process')?.judgment).toBe(
			'high_risk'
		);
		expect(suggestionForDomain(undefined, 'randomization_process')).toBeUndefined();
	});

	it('only counts a difference when both sides hold a judgment', () => {
		expect(differsFromSuggestion('high_risk', 'low_risk')).toBe(true);
		expect(differsFromSuggestion('high_risk', 'high_risk')).toBe(false);
		expect(differsFromSuggestion(null, 'low_risk')).toBe(false);
		expect(differsFromSuggestion('high_risk', undefined)).toBe(false);
		expect(differsFromSuggestion('high_risk', '')).toBe(false);
	});

	it('lists judgments that differ without a written reason', () => {
		const targets: JudgmentTarget[] = [
			{
				key: 'randomization_process',
				label: 'Randomization',
				suggested: 'high_risk',
				chosen: 'low_risk'
			},
			{
				key: 'reported_result',
				label: 'Reported result',
				suggested: 'low_risk',
				chosen: 'low_risk'
			},
			{ key: 'overall', label: 'Overall', suggested: 'high_risk', chosen: 'some_concerns' }
		];
		expect(targetsMissingReason(targets, {}).map((target) => target.key)).toEqual([
			'randomization_process',
			'overall'
		]);
		expect(
			targetsMissingReason(targets, {
				randomization_process: '  ',
				overall: 'Driven by the sequence.'
			}).map((target) => target.key)
		).toEqual(['randomization_process']);
	});

	it('sends only trimmed, non-empty reasons', () => {
		expect(
			presentOverrideReasons({ overall: '  Reason  ', reported_result: '   ', domain: '' })
		).toEqual({
			overall: 'Reason'
		});
	});

	it('labels judgments and drivers for reviewers', () => {
		const schema = {
			options: [
				{ value: 'low_risk', label: 'Low risk' },
				{ value: 'high_risk', label: 'High risk' }
			],
			allow_custom: false,
			required: true
		};
		expect(judgmentLabel(schema, 'high_risk')).toBe('High risk');
		expect(judgmentLabel(schema, 'unknown')).toBe('unknown');
		expect(judgmentLabel(schema, '')).toBeUndefined();
		expect(driverLabels(definition, ['sequence_unpredictable', 'gone'])).toEqual([
			'Was the allocation sequence chance-based?',
			'gone'
		]);
	});

	it('shortens long question wording at a word boundary', () => {
		const label =
			'Did departures from the planned intervention, such as crossover, co-interventions or poor adherence, differ between arms?';
		const short = shortLabel(label, 60);
		expect(short.endsWith('…')).toBe(true);
		expect(short.length).toBeLessThanOrEqual(60);
		expect(label.startsWith(short.slice(0, -1))).toBe(true);
	});
});
