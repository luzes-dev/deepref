import { describe, expect, it } from 'vitest';
import type { NodeTypeDto } from '#lib/api/generated/models/index.js';
import {
	autoArrange,
	catalogIndex,
	composeQueryTerms,
	describeCondition,
	describeSchedule,
	describeTrigger,
	detailToken,
	flowToGraph,
	graphToFlow,
	insertDetail,
	newNodeId,
	portsCompatible,
	previewForRun,
	refusalReason,
	summarizeNode,
	type WfGraph
} from './model';

const port = (id: string, type: string, label = id) => ({
	id,
	label,
	type,
	required: false,
	multiple: false
});

const filterDef: NodeTypeDto = {
	id: 'logic.filter',
	category: 'logic',
	label: 'Keep only some records',
	description: 'Keeps the records that match your rule.',
	inputs: [port('records', 'records')],
	outputs: [port('records', 'records', 'Kept'), port('rejected', 'records', 'Set aside')],
	config: [],
	has_side_effects: false,
	branches: false
};

describe('portsCompatible', () => {
	it('matches the server rules', () => {
		expect(portsCompatible('records', 'records')).toBe(true);
		expect(portsCompatible('record', 'records')).toBe(true);
		expect(portsCompatible('report', 'json')).toBe(true);
		expect(portsCompatible('records', 'record')).toBe(false);
		expect(portsCompatible('text', 'records')).toBe(false);
		expect(portsCompatible('study', 'document')).toBe(false);
	});

	it('explains refusals in plain words', () => {
		const message = refusalReason(port('records', 'records'), port('item', 'record', 'Record'));
		expect(message).toContain('for each record');
		expect(refusalReason(port('a', 'study'), port('b', 'document', 'Full text'))).toBe(
			'This step gives a study, but "Full text" needs a full text.'
		);
	});
});

describe('describeSchedule', () => {
	it('writes friendly sentences', () => {
		expect(describeSchedule({ every: 'day', at: '09:00' })).toBe('Every day at 09:00');
		expect(
			describeSchedule({ every: 'week', at: '09:00', weekdays: ['monday', 'thursday'] })
		).toBe('Every Monday and Thursday at 09:00');
		expect(
			describeSchedule({
				every: 'month',
				at: '07:30',
				day_of_month: 22,
				timezone: 'Europe/Lisbon'
			})
		).toBe('On the 22nd of every month at 07:30 (Europe/Lisbon)');
		expect(describeSchedule(undefined)).toBe('On a schedule');
	});
});

describe('publication search', () => {
	it('composes keywords, authors and journal', () => {
		expect(
			composeQueryTerms({
				keywords: ['sleep', 'deep learning'],
				authors: ['Li'],
				journal: 'Nature'
			})
		).toBe('(sleep OR "deep learning") AND (Li[Author]) AND Nature[Journal]');
		expect(composeQueryTerms({})).toBe('');
	});
});

describe('summaries', () => {
	it('describes rules without showing internals', () => {
		expect(
			describeCondition({
				match: 'any',
				rules: [
					{ field: 'year', operator: 'greater_than', value: 2020 },
					{ field: 'title', operator: 'contains', value: 'sleep' }
				]
			})
		).toBe('year is more than "2020" or title contains "sleep"');
		expect(summarizeNode(filterDef, {})).toBe('Build a rule');
	});

	it('describes the trigger of a workflow', () => {
		const graph: WfGraph = {
			nodes: [
				{
					id: 't',
					type: 'trigger.schedule',
					position: { x: 0, y: 0 },
					config: { schedule: { every: 'week', weekdays: ['monday'], at: '09:00' } }
				}
			],
			edges: []
		};
		expect(describeTrigger(graph)).toBe('Every Monday at 09:00');
		expect(describeTrigger({ nodes: [], edges: [] })).toBe('No trigger yet');
	});
});

describe('graph <-> canvas', () => {
	const graph: WfGraph = {
		nodes: [
			{
				id: 'a',
				type: 'logic.filter',
				position: { x: 10, y: 20 },
				label: 'Recent',
				config: { x: 1 }
			},
			{ id: 'b', type: 'logic.filter', position: { x: 300, y: 20 }, config: {} }
		],
		edges: [
			{ id: 'e1', from: { node: 'a', port: 'records' }, to: { node: 'b', port: 'records' } }
		]
	};

	it('round-trips', () => {
		const flow = graphToFlow(graph, catalogIndex([filterDef]));
		expect(flow.nodes[0].data.title).toBe('Recent');
		expect(flow.edges[0]).toMatchObject({ source: 'a', sourceHandle: 'records', target: 'b' });
		expect(flowToGraph(flow.nodes, flow.edges)).toEqual(graph);
	});

	it('drops notes and dangling edges', () => {
		const flow = graphToFlow(graph, catalogIndex([filterDef]));
		const nodes = [...flow.nodes.slice(0, 1), { ...flow.nodes[1], type: 'note' }];
		const result = flowToGraph(nodes, flow.edges);
		expect(result.nodes).toHaveLength(1);
		expect(result.edges).toHaveLength(0);
	});

	it('creates unique ids and arranges left to right', () => {
		expect(newNodeId('logic.filter', ['filter_1', 'filter_2'])).toBe('filter_3');
		const arranged = autoArrange(
			[
				{ id: 'a', position: { x: 500, y: 500 } },
				{ id: 'b', position: { x: 0, y: 0 } }
			],
			[{ source: 'a', target: 'b' }]
		);
		expect(arranged.find((n) => n.id === 'a')!.position.x).toBeLessThan(
			arranged.find((n) => n.id === 'b')!.position.x
		);
	});
});

describe('previewForRun', () => {
	it('summarises branches', () => {
		expect(
			previewForRun(filterDef, [
				{
					node_id: 'a',
					status: 'completed',
					input: { records: new Array(12).fill({}) },
					output: { records: new Array(9).fill({}), rejected: new Array(3).fill({}) }
				}
			])
		).toBe('12 in → 9 kept / 3 set aside');
	});

	it('shows the verdict counts of AI screening, and its progress while it waits', () => {
		const screening: NodeTypeDto = {
			...filterDef,
			id: 'ai.run_review',
			outputs: [
				port('records', 'records', 'All records'),
				port('included', 'records', 'AI would include'),
				port('excluded', 'records', 'AI would exclude'),
				port('unsure', 'records', 'Not sure')
			]
		};
		expect(
			previewForRun(screening, [
				{
					node_id: 'screen',
					status: 'completed',
					note: 'AI verdicts: 3 would include, 5 would exclude, 2 not sure.',
					input: { records: new Array(10).fill({}) },
					output: { records: new Array(10).fill({}), included: [{}, {}, {}] }
				}
			])
		).toBe('AI verdicts: 3 would include, 5 would exclude, 2 not sure.');
		expect(
			previewForRun(screening, [
				{
					node_id: 'screen',
					status: 'queued',
					note: 'Waiting for the AI to review 10 record(s): 4 of 10 finished so far.'
				}
			])
		).toBe('Waiting for the AI to review 10 record(s): 4 of 10 finished so far.');
	});
});

describe('insertDetail', () => {
	it('writes the token where the caret is', () => {
		expect(detailToken('ai_decision')).toBe('{{ai_decision}}');
		expect(insertDetail('Found  new', 'count', 6, 6)).toEqual({
			text: 'Found {{count}} new',
			caret: 15
		});
	});

	it('replaces a selection and falls back to the end when there is no caret', () => {
		expect(insertDetail('Hello title', 'title', 6, 11)).toEqual({
			text: 'Hello {{title}}',
			caret: 15
		});
		expect(insertDetail('Subject: ', 'subject', null, null)).toEqual({
			text: 'Subject: {{subject}}',
			caret: 20
		});
	});

	it('keeps positions inside the text', () => {
		expect(insertDetail('ab', 'x', 99, 99).text).toBe('ab{{x}}');
		expect(insertDetail('ab', 'x', -4, 1).text).toBe('{{x}}b');
	});
});
