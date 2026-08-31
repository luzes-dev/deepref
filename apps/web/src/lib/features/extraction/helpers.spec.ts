import { describe, expect, it } from 'vitest';
import type {
	AiExtractedFieldDto,
	AiExtractionEvidenceDto,
	ExtractionFieldDto
} from '$lib/api/generated/models';
import {
	buildExtractionEvidenceLink,
	draftFromAiField,
	serializeExtractionDrafts,
	validateExtractionDrafts
} from './helpers';
import type { ExtractionDraftField } from './helpers';

const fields: ExtractionFieldDto[] = [
	{
		id: 'field-number',
		project_id: 'project-1',
		field_key: 'sample_size',
		label: 'Sample size',
		value_type: 'number',
		required: true,
		version: 2
	},
	{
		id: 'field-note',
		project_id: 'project-1',
		field_key: 'note',
		label: 'Note',
		value_type: 'text',
		required: false,
		version: 1
	}
];

const evidence: AiExtractionEvidenceDto = {
	report_id: 'report-1',
	document_id: 'document-1',
	document_block_id: 'block-7',
	page: 4,
	parser_version: 'parser.v2',
	content_hash: 'hash-7'
};

type ValueDraft = Extract<ExtractionDraftField, { kind: 'value' }>;

function valueDraft(
	fieldId: string,
	fieldVersion: number,
	value: ValueDraft['value'],
	rationale = 'A value.'
): ValueDraft {
	return {
		field_id: fieldId,
		field_version: fieldVersion,
		kind: 'value',
		rationale,
		source: evidence,
		value
	};
}

function insufficientDraft(
	fieldId: string,
	fieldVersion: number,
	rationale = 'Not found.'
): Extract<ExtractionDraftField, { kind: 'insufficient_evidence' }> {
	return {
		field_id: fieldId,
		field_version: fieldVersion,
		kind: 'insufficient_evidence',
		rationale
	};
}

function expectValidationError(
	testFields: ExtractionFieldDto[],
	drafts: ExtractionDraftField[],
	message: string
): void {
	expect(validateExtractionDrafts(testFields, drafts)).toContain(message);
}

describe('extraction helpers', () => {
	it('serializes typed values while preserving exact field identity and provenance', () => {
		const proposalFields: AiExtractedFieldDto[] = [
			{
				field_id: 'field-number',
				field_version: 2,
				kind: 'value',
				rationale: 'The table reports the enrolled population.',
				source: evidence,
				value: { kind: 'number', value: 42 }
			},
			{
				field_id: 'field-note',
				field_version: 1,
				kind: 'insufficient_evidence',
				rationale: 'The report does not state this detail.'
			}
		];
		const drafts = proposalFields.map(draftFromAiField);
		if (drafts[0]?.kind !== 'value') throw new Error('expected value draft');
		drafts[0].value = { kind: 'number', value: '84.5' };

		const validation = validateExtractionDrafts(fields, drafts);
		expect(validation).toBeUndefined();
		const result = serializeExtractionDrafts(drafts);
		expect(result).toEqual({
			ok: true,
			fields: [
				{
					field_id: 'field-number',
					field_version: 2,
					kind: 'value',
					rationale: 'The table reports the enrolled population.',
					source: evidence,
					value: { kind: 'number', value: 84.5 }
				},
				{
					field_id: 'field-note',
					field_version: 1,
					kind: 'insufficient_evidence',
					rationale: 'The report does not state this detail.'
				}
			]
		});
	});

	it('rejects required insufficiency', () => {
		expectValidationError(
			fields,
			[insufficientDraft('field-number', 2), insufficientDraft('field-note', 1)],
			'Required'
		);
	});

	it('rejects non-finite numbers and invalid dates', () => {
		const numberDraft = valueDraft('field-number', 2, { kind: 'number', value: 'Infinity' });
		const noteDraft = valueDraft('field-note', 1, { kind: 'text', value: 'ok' });
		expectValidationError(fields, [numberDraft, noteDraft], 'finite');

		const validNumberDraft = valueDraft('field-number', 2, { kind: 'number', value: '84' });
		const dateField: ExtractionFieldDto = {
			id: 'field-date',
			project_id: 'project-1',
			field_key: 'publication_date',
			label: 'Publication date',
			value_type: 'date',
			required: false,
			version: 1
		};
		const invalidDateDraft = valueDraft(
			dateField.id,
			dateField.version,
			{
				kind: 'date',
				value: '2026-02-30'
			},
			'A date.'
		);
		expectValidationError(
			[...fields, dateField],
			[validNumberDraft, noteDraft, invalidDateDraft],
			'ISO date'
		);
	});

	it('rejects blank values and stale versions', () => {
		const numberDraft = valueDraft('field-number', 2, { kind: 'number', value: '84' });
		const noteDraft = valueDraft('field-note', 1, { kind: 'text', value: 'ok' });
		expectValidationError(
			fields,
			[valueDraft('field-number', 2, { kind: 'number', value: '   ' }), noteDraft],
			'finite'
		);
		expectValidationError(
			fields,
			[numberDraft, valueDraft('field-note', 1, { kind: 'text', value: '   ' })],
			'blank'
		);
		expectValidationError(
			fields,
			[valueDraft('field-number', 1, { kind: 'number', value: '84' }), noteDraft],
			'changed'
		);
	});

	it('preserves identity and validation error ordering', () => {
		const validNumberDraft = valueDraft('field-number', 2, { kind: 'number', value: '84' });
		const validNoteDraft = valueDraft('field-note', 1, { kind: 'text', value: 'ok' });
		expectValidationError(
			fields,
			[validNumberDraft, validNumberDraft],
			'appears more than once'
		);
		expectValidationError(
			fields,
			[validNumberDraft, valueDraft('field-other', 1, { kind: 'text', value: 'ok' })],
			'no longer current'
		);
		expectValidationError(
			fields,
			[validNumberDraft, insufficientDraft('field-note', 1, '')],
			'Add a rationale'
		);
		expectValidationError(
			fields,
			[valueDraft('field-number', 2, { kind: 'text', value: '84' }), validNoteDraft],
			'expects number'
		);
		expectValidationError(
			[...fields, { ...fields[0], id: 'field-unsupported', value_type: 'currency' }],
			[validNumberDraft, validNoteDraft],
			'one reviewed value'
		);
		expectValidationError(
			[...fields, { ...fields[0], id: 'field-unsupported', value_type: 'currency' }],
			[
				validNumberDraft,
				validNoteDraft,
				valueDraft('field-unsupported', 2, { kind: 'number', value: '1' })
			],
			'unsupported value type'
		);
		expectValidationError(
			[...fields, { ...fields[0], id: 'field-required', required: true }],
			[validNumberDraft, validNoteDraft, insufficientDraft('field-required', 2)],
			'cannot be marked insufficient'
		);
		expectValidationError(fields, [validNumberDraft], 'one reviewed value');
		expectValidationError(
			fields,
			[validNumberDraft, insufficientDraft('field-note', 1, '')],
			'Add a rationale'
		);
	});

	it('creates the established full-text report/page/block link', () => {
		expect(buildExtractionEvidenceLink('project-1', evidence)).toBe(
			'/projects/project-1/screening/full-text?report=report-1&page=4&block=block-7'
		);
	});
});
