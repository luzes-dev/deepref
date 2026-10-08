import { describe, expect, it } from 'vitest';
import type { ProtocolDto } from './api';
import { diffProtocols } from './diff';

function protocol(overrides: Partial<ProtocolDto> = {}): ProtocolDto {
	return {
		id: 'p1',
		project_id: 'proj',
		version: 1,
		name: 'Review',
		status: 'published',
		framework_kind: 'pico',
		framework_fields: { population: 'Adults', intervention: 'Exercise', outcome: 'Sleep' },
		objective: 'Objective',
		question: 'Question?',
		criteria: [
			{
				id: 'c1',
				kind: 'inclusion',
				stage: 'both',
				dimension: 'population',
				label: 'Adults',
				description: 'Aged 18+',
				ordinal: 0
			}
		],
		revision: 2,
		amendment_of: null,
		published_at: null,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z',
		...overrides
	};
}

describe('diffProtocols', () => {
	it('reports no changes for identical versions', () => {
		expect(diffProtocols(protocol(), protocol())).toEqual([]);
	});

	it('reports text, framework and criteria changes', () => {
		const next = protocol({
			question: 'Another question?',
			framework_fields: { population: 'Adults', intervention: 'Exercise', outcome: 'Mood' },
			criteria: [
				{
					id: 'c2',
					kind: 'exclusion',
					stage: 'full_text',
					dimension: 'design',
					label: 'Case reports',
					description: 'Single case',
					ordinal: 0
				}
			]
		});
		expect(diffProtocols(protocol(), next)).toEqual([
			{ kind: 'changed', subject: 'Question' },
			{ kind: 'changed', subject: 'Outcome' },
			{ kind: 'added', subject: 'Exclusion criterion: Case reports' },
			{ kind: 'removed', subject: 'Inclusion criterion: Adults' }
		]);
	});
});
