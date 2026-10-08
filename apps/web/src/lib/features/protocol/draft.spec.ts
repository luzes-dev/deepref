import { describe, expect, it } from 'vitest';
import type { ProtocolDto } from './api';
import {
	buildSaveProtocolRequest,
	emptyProtocolDraft,
	newProtocolDraft,
	protocolDraftFromDto,
	protocolIssues,
	restoreStoredDraft,
	serializeStoredDraft,
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

function protocolDto(overrides: Partial<ProtocolDto> = {}): ProtocolDto {
	return {
		id: 'protocol-1',
		project_id: 'project-1',
		version: 2,
		revision: 3,
		status: 'draft',
		name: 'Review protocol',
		objective: 'Evaluate outcomes',
		question: 'Does it work?',
		framework_kind: 'pico',
		framework_fields: { population: 'Adults', outcome: 'Activity' },
		criteria: [
			{
				id: 'criterion-1',
				ordinal: 0,
				kind: 'inclusion',
				stage: 'both',
				dimension: 'population',
				label: 'Adults',
				description: 'Adult participants'
			}
		],
		created_at: '2026-10-01T00:00:00Z',
		updated_at: '2026-10-01T00:00:00Z',
		...overrides
	};
}

describe('protocol draft from the server', () => {
	it('maps a framework protocol and keeps a snapshot of its fields', () => {
		const draft = protocolDraftFromDto(protocolDto(), nextClientId);
		expect(draft.frameworkKind).toBe('pico');
		expect(draft.frameworkFields.population).toBe('Adults');
		expect(draft.frameworkFieldSnapshots.pico?.population).toBe('Adults');
		expect(draft.customFrameworkFields).toEqual([]);
		expect(draft.criteria).toHaveLength(1);
		expect(draft.criteria[0]?.clientId).toBe('criterion-1');
	});

	it('turns an unknown framework into custom fields and drops malformed criteria', () => {
		const draft = protocolDraftFromDto(
			protocolDto({
				framework_kind: 'unheard-of' as ProtocolDto['framework_kind'],
				framework_fields: { setting: 'Hospital' },
				criteria: [
					{ ...protocolDto().criteria[0]!, kind: 'sometimes' as 'inclusion' },
					protocolDto().criteria[0]!
				]
			}),
			nextClientId
		);
		expect(draft.frameworkKind).toBe('custom');
		expect(draft.customFrameworkFields.map((field) => field.key)).toEqual(['setting']);
		expect(draft.customFrameworkSnapshot).toHaveLength(1);
		expect(draft.criteria).toHaveLength(1);
	});
});

describe('stored protocol drafts', () => {
	const base = () => protocolDraftFromDto(protocolDto(), nextClientId);

	it('restores an edited draft and the amending flag', () => {
		const edited = base();
		edited.question = 'Does it work for older adults?';
		edited.criteria = [...edited.criteria, { ...criterion('exclusion'), id: undefined }];
		const restored = restoreStoredDraft(
			serializeStoredDraft(edited, true),
			base(),
			nextClientId
		);
		expect(restored?.amending).toBe(true);
		expect(restored?.draft.question).toBe('Does it work for older adults?');
		expect(restored?.draft.criteria.map((item) => item.kind)).toEqual([
			'inclusion',
			'exclusion'
		]);
		expect(restored?.draft.criteria[1]?.clientId).toBe('criterion-test');
	});

	it('restores custom framework fields only for a custom draft', () => {
		const edited = base();
		edited.frameworkKind = 'custom';
		edited.customFrameworkFields = [{ clientId: 'field-1', key: 'setting', value: 'Hospital' }];
		const restored = restoreStoredDraft(
			serializeStoredDraft(edited, false),
			base(),
			nextClientId
		);
		expect(restored?.amending).toBe(false);
		expect(restored?.draft.customFrameworkFields).toEqual([
			{ clientId: 'field-test', key: 'setting', value: 'Hospital' }
		]);
	});

	it('ignores a stored draft that matches the server draft', () => {
		expect(restoreStoredDraft(serializeStoredDraft(base(), false), base(), nextClientId)).toBe(
			undefined
		);
	});

	it.each([
		['not json', '{'],
		['no values', JSON.stringify({ amending: false })],
		[
			'missing name',
			JSON.stringify({ values: { objective: '', question: '', frameworkKind: 'pico' } })
		],
		[
			'unknown framework',
			JSON.stringify({
				values: { name: '', objective: '', question: '', frameworkKind: 'nope' }
			})
		],
		[
			'no framework',
			JSON.stringify({
				values: { name: 'x', objective: '', question: '', frameworkKind: '' }
			})
		]
	])('rejects a stored draft with %s', (_, raw) => {
		expect(restoreStoredDraft(raw, base(), nextClientId)).toBe(undefined);
	});
});
