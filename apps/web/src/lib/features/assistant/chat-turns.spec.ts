import { describe, expect, it } from 'vitest';
import type { AssistantChatStreamEvent } from '#lib/api/assistant-stream.js';
import type { AssistantMessageRecord } from './chat-api';
import {
	STOPPED_NOTE,
	decodeEntities,
	emptyAssistantTurn,
	harvestCitations,
	markStopped,
	recordToTurn,
	reduceAssistantTurn,
	type AssistantTurn
} from './chat-turns';

const PROJECT = 'p1';

function apply(turn: AssistantTurn, events: AssistantChatStreamEvent[]): AssistantTurn {
	return events.reduce((current, event) => reduceAssistantTurn(current, event, PROJECT), turn);
}

describe('reduceAssistantTurn', () => {
	it('appends streamed tokens in arrival order', () => {
		const turn = apply(emptyAssistantTurn('t1'), [
			{ event: 'token', delta: 'There are ' },
			{ event: 'token', delta: '**50 records**' }
		]);
		expect(turn.content).toBe('There are **50 records**');
	});

	it('restarts the text when a tool starts, so only the final answer is kept', () => {
		const turn = apply(emptyAssistantTurn('t1'), [
			{ event: 'token', delta: 'Let me check. ' },
			{ event: 'tool_start', tool: 'get_project_overview', tool_call_id: 'c1', args: {} },
			{
				event: 'tool_complete',
				tool: 'get_project_overview',
				tool_call_id: 'c1',
				output: {}
			},
			{ event: 'token', delta: 'Done.' }
		]);
		expect(turn.content).toBe('Done.');
		expect(turn.tools).toEqual([
			{
				toolCallId: 'c1',
				tool: 'get_project_overview',
				args: {},
				status: 'completed',
				output: {}
			}
		]);
	});

	it('replaces the text with the stored reply when the server says so', () => {
		const turn = apply(emptyAssistantTurn('t1'), [
			{ event: 'token', delta: 'Partly wrong' },
			{ event: 'replace', text: 'The reply.' }
		]);
		expect(turn.content).toBe('The reply.');
	});

	it('shows a running tool as running until its result arrives', () => {
		const started = apply(emptyAssistantTurn('t1'), [
			{
				event: 'tool_start',
				tool: 'search_project_reports',
				tool_call_id: 'c2',
				args: { query: 'fitbit' }
			}
		]);
		expect(started.tools[0]?.status).toBe('running');
	});

	it('records the usage from done and keeps the plan when one is proposed', () => {
		const plan = {
			id: 'plan-1',
			project_id: PROJECT,
			conversation_id: 'conv',
			status: 'pending',
			summary: 'Exclude 2',
			actions: [],
			results: null,
			error: null,
			model: 'glm-5.3-flash',
			prompt_version: 'v2',
			created_at: '2026-10-08T12:00:00Z',
			resolved_by: null,
			resolved_at: null
		};
		const turn = apply(emptyAssistantTurn('t1'), [
			{ event: 'plan', plan },
			{ event: 'done', message_id: 'm1', input_tokens: 10, output_tokens: 4 }
		]);
		expect(turn.planId).toBe('plan-1');
		expect(turn.tokens).toEqual({ input: 10, output: 4 });
	});
});

describe('markStopped', () => {
	it('keeps the partial answer and adds the note the server stores', () => {
		const turn = markStopped(
			apply(emptyAssistantTurn('t1'), [{ event: 'token', delta: 'The first two ' }])
		);
		expect(turn.stopped).toBe(true);
		expect(turn.content).toBe(`The first two\n\n${STOPPED_NOTE}`);
	});

	it('shows only the note when nothing had streamed yet', () => {
		expect(markStopped(emptyAssistantTurn('t1')).content).toBe(STOPPED_NOTE);
	});

	it('marks tools that were still running as failed', () => {
		const turn = markStopped(
			apply(emptyAssistantTurn('t1'), [
				{ event: 'tool_start', tool: 'get_report', tool_call_id: 'c1', args: {} }
			])
		);
		expect(turn.tools[0]?.status).toBe('failed');
	});
});

describe('citations', () => {
	it('labels a report with its title and year and links it to the articles view', () => {
		const [citation] = harvestCitations(
			'get_report',
			{
				id: '069861a2-0000-4000-8000-000000000001',
				title: 'Perceptual-Auditory and Acoustic Analysis of Breathiness in Cis and Transgender Men and Women',
				publication_year: 2016
			},
			PROJECT
		);
		// The year leads, so it survives the pill truncating the title.
		expect(citation?.label).toBe('2016 · Perceptual-Auditory and Acoustic…');
		expect(citation?.href).toBe(
			'/projects/p1/articles?report=069861a2-0000-4000-8000-000000000001'
		);
		expect(citation?.tooltip).toContain('069861a2-0000-4000-8000-000000000001');
		expect(citation?.label).not.toContain('069861a2');
	});

	it('links a study to the studies view and the protocol to the protocol page', () => {
		const [study] = harvestCitations('get_study', { id: 's1', title: 'Fitbit pilot' }, PROJECT);
		expect(study).toMatchObject({
			label: 'Fitbit pilot',
			href: '/projects/p1/studies?study=s1'
		});
		const [protocol] = harvestCitations(
			'get_project_protocol',
			{ id: 'pr1', name: 'Protocol v2' },
			PROJECT
		);
		expect(protocol).toMatchObject({ label: 'Protocol v2', href: '/projects/p1/protocol' });
	});

	it('labels a passage by its page without a link, because no passage route exists', () => {
		const [passage] = harvestCitations(
			'read_document_blocks',
			[{ id: 'b1', page_number: 4, text: 'Randomised 60 women.' }],
			PROJECT
		);
		expect(passage).toMatchObject({ label: 'Passage, page 4', href: null });
		expect(passage?.tooltip).toContain('b1');
	});

	it('decodes HTML entities that the imports left in titles', () => {
		const [citation] = harvestCitations(
			'search_project_reports',
			[{ id: 'r9', title: 'Lancet Diabetes &amp; Endocrinology', publication_year: 2024 }],
			PROJECT
		);
		expect(citation?.label).toBe('2024 · Lancet Diabetes & Endocrinology');
	});

	it('collects each record once and keeps at most eight across tool calls', () => {
		let turn = emptyAssistantTurn('t1');
		for (let index = 0; index < 4; index += 1) {
			const output = [0, 1, 2].map((position) => ({
				id: `a${index}-${position}`,
				title: `Paper ${index}.${position}`
			}));
			// Repeating the first record must not add a second pill.
			turn = apply(turn, [
				{
					event: 'tool_complete',
					tool: 'search_project_reports',
					tool_call_id: `c${index}`,
					output: [...output, output[0]]
				}
			]);
		}
		// Twelve distinct records were found; the answer shows at most eight.
		expect(turn.citations).toHaveLength(8);
		expect(new Set(turn.citations.map((citation) => citation.key)).size).toBe(8);
	});

	it('decodes entities without decoding them twice', () => {
		expect(decodeEntities('A &amp;lt; B &amp; C &#39;x&#39;')).toBe("A &lt; B & C 'x'");
	});
});

describe('recordToTurn', () => {
	it('maps a stored assistant message with its tools, citations and stop flag', () => {
		const record: AssistantMessageRecord = {
			id: 'm1',
			conversation_id: 'conv',
			role: 'assistant',
			content: `Partly.\n\n${STOPPED_NOTE}`,
			tool_calls: [{ id: 'c1', tool: 'get_report', args: {} }],
			tool_results: [
				{
					tool_call_id: 'c1',
					tool: 'get_report',
					output: { id: 'r1', title: 'Fitbit RCT', publication_year: 2019 }
				}
			],
			metadata: { stopped: true, input_tokens: 5, output_tokens: 2, plan_id: null },
			created_at: '2026-10-08T12:00:00Z'
		};
		const turn = recordToTurn(record, PROJECT);
		expect(turn).toMatchObject({
			kind: 'assistant',
			stopped: true,
			tokens: { input: 5, output: 2 },
			planId: null,
			tools: [{ toolCallId: 'c1', status: 'completed' }],
			citations: [
				{ key: 'r1', label: '2019 · Fitbit RCT', href: '/projects/p1/articles?report=r1' }
			]
		});
	});

	it('maps a user message as a user turn', () => {
		const record: AssistantMessageRecord = {
			id: 'u1',
			conversation_id: 'conv',
			role: 'user',
			content: 'How many?',
			tool_calls: null,
			tool_results: null,
			metadata: null,
			created_at: '2026-10-08T12:00:00Z'
		};
		expect(recordToTurn(record, PROJECT)).toEqual({
			kind: 'user',
			id: 'u1',
			content: 'How many?',
			createdAt: '2026-10-08T12:00:00Z'
		});
	});
});
