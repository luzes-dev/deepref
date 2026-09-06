import { describe, expect, it } from 'vitest';
import {
	buildSaveProtocolRequest,
	emptyProtocolDraft,
	validateProtocolDraft,
	type DraftClientIdFactory
} from './draft';

const nextClientId: DraftClientIdFactory = (kind) => `${kind}-test`;

describe('protocol draft model', () => {
	it('serializes and trims a valid custom protocol draft', () => {
		const draft = emptyProtocolDraft();
		draft.id = 'protocol-version';
		draft.revision = 4;
		draft.name = ' Review protocol ';
		draft.objective = ' Evaluate outcomes ';
		draft.question = ' Does it work? ';
		draft.frameworkKind = 'custom';
		draft.frameworkFields = {};
		draft.customFrameworkFields = [
			{ clientId: nextClientId('field'), key: ' population ', value: ' Adults ' }
		];
		draft.criteria = [
			{
				clientId: nextClientId('criterion'),
				kind: 'inclusion',
				stage: 'both',
				dimension: 'population',
				label: ' Eligible ',
				description: ' Adult participants '
			}
		];

		expect(validateProtocolDraft(draft)).toEqual([]);
		expect(buildSaveProtocolRequest(draft)).toEqual({
			name: 'Review protocol',
			objective: 'Evaluate outcomes',
			question: 'Does it work?',
			framework: { kind: 'custom', fields: { population: 'Adults' } },
			criteria: [
				{
					kind: 'inclusion',
					stage: 'both',
					dimension: 'population',
					label: 'Eligible',
					description: 'Adult participants'
				}
			],
			protocol_version_id: 'protocol-version',
			expected_revision: 4
		});
	});

	it('keeps protocol validation outside the Svelte editor', () => {
		const draft = emptyProtocolDraft();
		draft.frameworkKind = 'custom';
		draft.frameworkFields = {};
		draft.customFrameworkFields = [
			{ clientId: 'field-1', key: 'scope', value: 'one' },
			{ clientId: 'field-2', key: ' scope ', value: 'two' }
		];

		const errors = validateProtocolDraft(draft);
		expect(errors).toContain('Give the protocol a name.');
		expect(errors).toContain('Add the review objective.');
		expect(errors).toContain('Add the research question.');
		expect(errors).toContain('Custom framework field names must be unique: scope.');
	});
});
