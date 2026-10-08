import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	AssistantStreamError,
	parseSseFrame,
	streamAssistantChat,
	type AssistantChatStreamEvent
} from './assistant-stream';

function frame(event: string, data: unknown): string {
	return `event: ${event}\ndata: ${JSON.stringify(data)}`;
}

function sseBody(chunks: string[]): ReadableStream<Uint8Array> {
	const encoder = new TextEncoder();
	return new ReadableStream({
		start(controller) {
			for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
			controller.close();
		}
	});
}

async function collect(response: Response): Promise<AssistantChatStreamEvent[]> {
	vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response));
	const events: AssistantChatStreamEvent[] = [];
	await streamAssistantChat('project-1', { conversation_id: 'c1', message: 'hi' }, (event) =>
		events.push(event)
	);
	return events;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('parseSseFrame', () => {
	it.each<[string, unknown, AssistantChatStreamEvent | null]>([
		['token', { delta: 'Hel' }, { event: 'token', delta: 'Hel' }],
		['replace', { text: 'Hello' }, { event: 'replace', text: 'Hello' }],
		['status', { message: 'Reading' }, { event: 'status', message: 'Reading' }],
		[
			'tool_start',
			{ tool: 'search', tool_call_id: 't1', args: { q: 'x' } },
			{ event: 'tool_start', tool: 'search', tool_call_id: 't1', args: { q: 'x' } }
		],
		[
			'tool_start',
			{ tool: 'search', tool_call_id: 't1', args: 'bad' },
			{ event: 'tool_start', tool: 'search', tool_call_id: 't1', args: {} }
		],
		[
			'tool_complete',
			{ tool: 'search', tool_call_id: 't1' },
			{ event: 'tool_complete', tool: 'search', tool_call_id: 't1', output: null }
		],
		[
			'proposal_created',
			{ tool: 'screen', review_run_id: 'r1', status_path: '/runs/r1' },
			{
				event: 'proposal_created',
				tool: 'screen',
				review_run_id: 'r1',
				status_path: '/runs/r1'
			}
		],
		[
			'done',
			{ message_id: 'm1', input_tokens: 3, output_tokens: 'x' },
			{ event: 'done', message_id: 'm1', input_tokens: 3, output_tokens: 0 }
		],
		['plan', { plan: { steps: [] } }, { event: 'plan', plan: { steps: [] } }],
		['plan', { plan: 'later' }, null],
		['error', {}, { event: 'error', message: 'assistant turn failed', code: null }],
		[
			'error',
			{ message: 'Out of budget', code: 'budget_exceeded' },
			{ event: 'error', message: 'Out of budget', code: 'budget_exceeded' }
		],
		['toString', {}, null],
		['unknown', {}, null]
	])('normalizes a %s event', (event, data, expected) => {
		expect(parseSseFrame(frame(event, data))).toEqual(expected);
	});

	it('skips comments and frames without an event or JSON object', () => {
		expect(parseSseFrame(`: keep-alive\n${frame('token', { delta: 'a' })}`)).toEqual({
			event: 'token',
			delta: 'a'
		});
		expect(parseSseFrame('data: {"delta":"a"}')).toBeNull();
		expect(parseSseFrame('event: token\ndata: not json')).toBeNull();
		expect(parseSseFrame('event: token\ndata: 3')).toBeNull();
	});
});

describe('streamAssistantChat', () => {
	it('emits events split across chunks and a final frame without a blank line', async () => {
		const events = await collect(
			new Response(
				sseBody([
					`${frame('token', { delta: 'Hel' })}\r\n\r\n${frame('token', { delta: 'l' }).slice(0, 10)}`,
					`${frame('token', { delta: 'l' }).slice(10)}\n\n`,
					frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 })
				])
			)
		);
		expect(events.map((event) => event.event)).toEqual(['token', 'token', 'done']);
	});

	it('raises the API message and code for a failed request', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValue(
				new Response(
					JSON.stringify({ message: 'Budget reached', code: 'budget_exceeded' }),
					{
						status: 429
					}
				)
			)
		);
		const error = await streamAssistantChat(
			'project-1',
			{ conversation_id: 'c1', message: 'hi' },
			() => {}
		).catch((caught: unknown) => caught);
		expect(error).toBeInstanceOf(AssistantStreamError);
		expect(error).toMatchObject({
			status: 429,
			message: 'Budget reached',
			code: 'budget_exceeded'
		});
	});

	it('falls back to the status text when the error body is not JSON', async () => {
		vi.stubGlobal(
			'fetch',
			vi
				.fn()
				.mockResolvedValue(
					new Response('gateway down', { status: 502, statusText: 'Bad Gateway' })
				)
		);
		await expect(
			streamAssistantChat('project-1', { conversation_id: 'c1', message: 'hi' }, () => {})
		).rejects.toMatchObject({ status: 502, message: 'Bad Gateway', code: null });
	});

	it('fails when the response has no body', async () => {
		await expect(collect(new Response(null, { status: 200 }))).rejects.toThrow(
			'assistant stream is unavailable'
		);
	});
});
