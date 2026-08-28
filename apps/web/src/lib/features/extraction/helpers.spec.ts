import { describe, expect, it } from 'vitest';
import type { AiExtractedFieldDto, ExtractionFieldDto } from '$lib/api/generated/models';
import {
	buildExtractionEvidenceLink,
	draftFromAiField,
	serializeExtractionDrafts,
	validateExtractionDrafts
} from './helpers';

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

const evidence = {
	report_id: 'report-1',
	document_id: 'document-1',
	document_block_id: 'block-7',
	page: 4,
	parser_version: 'parser.v2',
	content_hash: 'hash-7'
};

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

	it('rejects required insufficiency, non-finite numbers, invalid dates, and stale versions', () => {
		const requiredInsufficient = [
			{
				field_id: 'field-number',
				field_version: 2,
				kind: 'insufficient_evidence' as const,
				rationale: 'Not found.'
			},
			{
				field_id: 'field-note',
				field_version: 1,
				kind: 'insufficient_evidence' as const,
				rationale: 'Not found.'
			}
		];
		expect(validateExtractionDrafts(fields, requiredInsufficient)).toContain('Required');

		const numberDraft = {
			field_id: 'field-number',
			field_version: 2,
			kind: 'value' as const,
			rationale: 'A table value.',
			source: evidence,
			value: { kind: 'number' as const, value: 'Infinity' }
		};
		const noteDraft = {
			field_id: 'field-note',
			field_version: 1,
			kind: 'value' as const,
			rationale: 'A note.',
			source: evidence,
			value: { kind: 'text' as const, value: 'ok' }
		};
		expect(validateExtractionDrafts(fields, [numberDraft, noteDraft])).toContain('finite');

		numberDraft.value = { kind: 'number', value: '84' };
		const dateField: ExtractionFieldDto = {
			id: 'field-date',
			project_id: 'project-1',
			field_key: 'publication_date',
			label: 'Publication date',
			value_type: 'date',
			required: false,
			version: 1
		};
		const invalidDateDraft = {
			field_id: dateField.id,
			field_version: dateField.version,
			kind: 'value' as const,
			rationale: 'A date.',
			source: evidence,
			value: { kind: 'date' as const, value: '2026-02-30' }
		};
		expect(
			validateExtractionDrafts(
				[...fields, dateField],
				[numberDraft, noteDraft, invalidDateDraft]
			)
		).toContain('ISO date');

		expect(
			validateExtractionDrafts(fields, [
				{ ...numberDraft, value: { kind: 'number', value: '   ' } },
				noteDraft
			])
		).toContain('finite');
		expect(
			validateExtractionDrafts(fields, [
				numberDraft,
				{ ...noteDraft, value: { kind: 'text', value: '   ' } }
			])
		).toContain('blank');

		expect(
			validateExtractionDrafts(fields, [{ ...numberDraft, field_version: 1 }, noteDraft])
		).toContain('changed');
	});

	it('creates the established full-text report/page/block link', () => {
		expect(buildExtractionEvidenceLink('project-1', evidence)).toBe(
			'/projects/project-1/screening/full-text?report=report-1&page=4&block=block-7'
		);
	});
});
