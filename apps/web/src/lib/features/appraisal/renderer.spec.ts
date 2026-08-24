import { describe, expect, it } from 'vitest';
import { defaultResponse, responseIsComplete, responseLabel } from './renderer';

describe('generic appraisal response transforms', () => {
	it('uses schema data for materially different defaults', () => {
		expect(defaultResponse({ kind: 'enum', options: [{ label: 'Low', value: 'low' }] })).toBe(
			'low'
		);
		expect(defaultResponse({ kind: 'scale', min: 1, max: 5, labels: {} })).toBeUndefined();
		expect(defaultResponse({ kind: 'text', max_length: 200 })).toBeUndefined();
	});

	it('validates required responses and resolves enum labels', () => {
		const question = {
			id: 'q1',
			label: 'Risk',
			required: true,
			requires_evidence: false,
			answer_schema: { kind: 'enum' as const, options: [{ label: 'Low', value: 'low' }] }
		};
		expect(responseIsComplete(question, {})).toBe(false);
		expect(responseIsComplete(question, { q1: 'low' })).toBe(true);
		expect(responseLabel(question.answer_schema, 'low')).toBe('Low');
	});
});
