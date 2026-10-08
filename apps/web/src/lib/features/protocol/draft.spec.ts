import { describe, expect, it } from 'vitest';
import {
	buildSaveProtocolRequest,
	emptyProtocolDraft,
	newProtocolDraft,
	protocolIssues,
	validateProtocolDraft,
	type DraftClientIdFactory,
	type ProtocolDraft
} from './draft';

const nextClientId: DraftClientIdFactory = (kind) => `${kind}-test`;

function completeDraft(): ProtocolDraft {
	const draft = emptyProtocolDraft();
	draft.name = 'Review protocol';
	draft.objective = 'Evaluate outcomes';
	draft.question = 'Does it work?';
	draft.frameworkFields = {
		population: 'Adults',
		intervention: 'Device',
		outcome: 'Activity'
	};
	return draft;
}

function criterion(kind: 'inclusion' | 'exclusion') {
	return {
		clientId: nextClientId('criterion'),
		kind,
		stage: 'both' as const,
		dimension: 'population' as const,
		label: 'Adults',
		description: 'Adult participants'
	};
}

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

	it('leaves blank optional framework fields out of the save payload', () => {
		const draft = completeDraft();
		expect(buildSaveProtocolRequest(draft).framework).toEqual({
			kind: 'pico',
			fields: { population: 'Adults', intervention: 'Device', outcome: 'Activity' }
		});
	});

	it('lets a draft without eligibility criteria be saved', () => {
		const draft = completeDraft();
		expect(draft.criteria).toEqual([]);
		expect(validateProtocolDraft(draft)).toEqual([]);
		expect(buildSaveProtocolRequest(draft).criteria).toEqual([]);
	});

	it('blocks publishing without an inclusion criterion and points at the criteria tab', () => {
		const withoutCriteria = protocolIssues(completeDraft());
		expect(withoutCriteria).toContainEqual({
			message: 'No inclusion criterion',
			tab: 'criteria',
			targetId: 'protocol-add-criterion'
		});

		const exclusionOnly = completeDraft();
		exclusionOnly.criteria = [criterion('exclusion')];
		expect(protocolIssues(exclusionOnly).map((issue) => issue.message)).toContain(
			'No inclusion criterion'
		);

		const publishable = completeDraft();
		publishable.criteria = [criterion('exclusion'), criterion('inclusion')];
		expect(protocolIssues(publishable).map((issue) => issue.message)).not.toContain(
			'No inclusion criterion'
		);
	});

	it('pre-fills a new protocol from the project name and description', () => {
		const draft = newProtocolDraft({
			name: ' Early mobilisation ',
			description: ' Does early mobilisation reduce delirium? '
		});
		expect(draft.name).toBe('Early mobilisation');
		expect(draft.question).toBe('Does early mobilisation reduce delirium?');
		expect(draft.objective).toBe('');
		expect(draft.revision).toBe(0);
	});

	it('keeps a blank question when the project has no description', () => {
		expect(newProtocolDraft({ name: 'Scoping review', description: null }).question).toBe('');
		expect(newProtocolDraft().name).toBe('');
	});
});
