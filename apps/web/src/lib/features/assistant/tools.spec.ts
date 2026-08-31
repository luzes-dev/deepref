import { describe, expect, it } from 'vitest';
import { AssistantToolKind } from '$lib/api/generated/models';
import type { AssistantToolDescriptor } from '$lib/api/generated/models';
import {
	ASSISTANT_TOOL_METADATA,
	ASSISTANT_TOOL_NAMES,
	initialToolValues,
	isAssistantToolName,
	partitionAssistantCatalog,
	reviewPath,
	serializeToolRequest,
	type ToolName,
	type ToolValues
} from './tools';

const PROJECT_ID = 'project-1';
const REPORT_ID = '11111111-1111-4111-8111-111111111111';
const DOCUMENT_ID = '22222222-2222-4222-8222-222222222222';
const BLOCK_ID = '33333333-3333-4333-8333-333333333333';
const STUDY_ID = '44444444-4444-4444-8444-444444444444';
const RECORD_ID = '55555555-5555-4555-8555-555555555555';
const CANDIDATE_REPORT_ID = '66666666-6666-4666-8666-666666666666';

const validValues = {
	get_project_protocol: {},
	get_report: { report_id: REPORT_ID },
	read_document_blocks: { document_id: DOCUMENT_ID, block_ids: BLOCK_ID },
	search_document: { document_id: DOCUMENT_ID, query: 'randomized trial', limit: '10' },
	search_project_reports: { query: 'randomized trial', limit: '10' },
	get_screening_state: { report_id: REPORT_ID },
	get_study: { study_id: STUDY_ID },
	get_appraisal: {
		report_id: REPORT_ID,
		definition_id: 'rob-2',
		definition_version: '2'
	},
	propose_screening_decision: { report_id: REPORT_ID, stage: 'full_text' },
	propose_duplicate_merge: {
		source_record_id: RECORD_ID,
		candidate_report_id: CANDIDATE_REPORT_ID
	},
	propose_study_grouping: { report_id: REPORT_ID },
	propose_classification: { study_id: STUDY_ID },
	propose_extraction: { study_id: STUDY_ID },
	propose_appraisal_answer: {
		report_id: REPORT_ID,
		definition_id: 'rob-2',
		definition_version: '1'
	}
} satisfies Record<ToolName, ToolValues>;

describe('project assistant tool metadata and request serialization', () => {
	it('keeps the exact fourteen-tool catalog split into reads and proposals', () => {
		expect(ASSISTANT_TOOL_NAMES).toHaveLength(14);
		expect(new Set(ASSISTANT_TOOL_NAMES).size).toBe(14);
		expect(Object.keys(ASSISTANT_TOOL_METADATA)).toHaveLength(14);
		expect(
			ASSISTANT_TOOL_NAMES.filter(
				(name) => ASSISTANT_TOOL_METADATA[name].kind === AssistantToolKind.read
			)
		).toHaveLength(8);
		expect(
			ASSISTANT_TOOL_NAMES.filter(
				(name) => ASSISTANT_TOOL_METADATA[name].kind === AssistantToolKind.proposal
			)
		).toHaveLength(6);
		expect(isAssistantToolName('search_document')).toBe(true);
		expect(isAssistantToolName('arbitrary_sql')).toBe(false);
	});

	it('serializes every exact tool with route project scope and typed values', () => {
		for (const tool of ASSISTANT_TOOL_NAMES) {
			const result = serializeToolRequest(tool, PROJECT_ID, validValues[tool]);
			expect(result.kind, tool).toBe('valid');
			if (result.kind !== 'valid') continue;
			expect(result.request.tool).toBe(tool);
			expect(result.request.args.project_id).toBe(PROJECT_ID);
		}

		const complexRead = serializeToolRequest(
			'read_document_blocks',
			PROJECT_ID,
			validValues.read_document_blocks
		);
		expect(complexRead).toEqual({
			kind: 'valid',
			request: {
				tool: 'read_document_blocks',
				args: {
					project_id: PROJECT_ID,
					document_id: DOCUMENT_ID,
					block_ids: [BLOCK_ID]
				}
			}
		});

		const proposal = serializeToolRequest(
			'propose_screening_decision',
			PROJECT_ID,
			validValues.propose_screening_decision
		);
		expect(proposal).toEqual({
			kind: 'valid',
			request: {
				tool: 'propose_screening_decision',
				args: { project_id: PROJECT_ID, report_id: REPORT_ID, stage: 'full_text' }
			}
		});
	});

	it('rejects malformed UUIDs, block lists, limits, versions, and text before sending', () => {
		const invalidUuid = serializeToolRequest('get_report', PROJECT_ID, {
			report_id: 'not-a-uuid'
		});
		expect(invalidUuid.kind).toBe('invalid');
		if (invalidUuid.kind === 'invalid') expect(invalidUuid.errors.report_id).toBeTruthy();

		const invalidBlocks = serializeToolRequest('read_document_blocks', PROJECT_ID, {
			document_id: DOCUMENT_ID,
			block_ids: `${BLOCK_ID}\nnot-a-uuid`
		});
		expect(invalidBlocks.kind).toBe('invalid');
		if (invalidBlocks.kind === 'invalid') expect(invalidBlocks.errors.block_ids).toBeTruthy();

		for (const limit of ['0', '101', '1.5', 'nope']) {
			const invalidLimit = serializeToolRequest('search_project_reports', PROJECT_ID, {
				query: 'trial',
				limit
			});
			expect(invalidLimit.kind).toBe('invalid');
		}

		const invalidVersion = serializeToolRequest('get_appraisal', PROJECT_ID, {
			report_id: REPORT_ID,
			definition_id: 'rob-2',
			definition_version: '0'
		});
		expect(invalidVersion.kind).toBe('invalid');

		const invalidText = serializeToolRequest('search_project_reports', PROJECT_ID, {
			query: '   ',
			limit: '10'
		});
		expect(invalidText.kind).toBe('invalid');
	});

	it('partitions unknown or mismatched server descriptors as unsupported', () => {
		const descriptor = (name: string, kind: 'read' | 'proposal'): AssistantToolDescriptor => ({
			name,
			kind,
			authority_tier: 'read_only',
			description: 'server description'
		});
		const partition = partitionAssistantCatalog([
			descriptor('get_report', 'read'),
			descriptor('get_report', 'proposal'),
			descriptor('future_tool', 'read')
		]);
		expect(partition.supported.map((entry) => entry.metadata.name)).toEqual(['get_report']);
		expect(partition.unsupported.map((entry) => entry.name)).toEqual([
			'get_report',
			'future_tool'
		]);
	});

	it('links proposal receipts to their human review queues', () => {
		expect(
			reviewPath(
				'propose_screening_decision',
				PROJECT_ID,
				validValues.propose_screening_decision
			)
		).toBe('/projects/project-1/screening/full-text');
		expect(
			reviewPath('propose_duplicate_merge', PROJECT_ID, validValues.propose_duplicate_merge)
		).toBe('/projects/project-1/discovery/duplicates');
		expect(
			reviewPath('propose_study_grouping', PROJECT_ID, validValues.propose_study_grouping)
		).toBe('/projects/project-1/studies?report=11111111-1111-4111-8111-111111111111');
		expect(
			reviewPath('propose_classification', PROJECT_ID, validValues.propose_classification)
		).toBe('/projects/project-1/studies?study=44444444-4444-4444-8444-444444444444');
		expect(reviewPath('get_report', PROJECT_ID, {})).toBeNull();
	});

	it('provides a deterministic form state for each tool', () => {
		for (const tool of ASSISTANT_TOOL_NAMES) {
			const values = initialToolValues(tool);
			expect(Object.keys(values)).toEqual(
				ASSISTANT_TOOL_METADATA[tool].fields.map((field) => field.key)
			);
		}
		expect(initialToolValues('search_document').limit).toBe('20');
		expect(initialToolValues('propose_screening_decision').stage).toBe('title_abstract');
	});
});
