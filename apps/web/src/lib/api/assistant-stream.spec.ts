import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	AssistantStreamError,
	getAssistantRun,
	parseSseFrame,
	streamAssistantChat,
	streamAssistantRunEvents,
	type AssistantChatStreamEvent
} from './assistant-stream';

function frame(event: string, data: unknown, seq?: number): string {
	const head = seq === undefined ? '' : `id: ${seq}\n`;
	return `${head}event: ${event}\ndata: ${JSON.stringify(data)}`;
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

	it('ignores the server seq id when normalizing events', () => {
		expect(parseSseFrame(`id: 7\n${frame('token', { delta: 'a' })}`)).toEqual({
			event: 'token',
			delta: 'a'
		});
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

describe('durable assistant runs', () => {
	const runId = 'run-1';

	function runJson(overrides: Record<string, unknown> = {}): Record<string, unknown> {
		return {
			id: runId,
			conversation_id: 'c1',
			project_id: 'project-1',
			trigger_message_id: 'trigger-1',
			status: 'completed',
			answer_message_id: 'answer-1',
			plan_id: null,
			error: {},
			created_at: '2026-01-01T00:00:00Z',
			updated_at: '2026-01-01T00:00:01Z',
			completed_at: '2026-01-01T00:00:02Z',
			...overrides
		};
	}

	function jsonResponse(body: unknown, status: number): Response {
		return new Response(JSON.stringify(body), {
			status,
			headers: { 'Content-Type': 'application/json' }
		});
	}

	function eventsResponse(frames: string[]): Response {
		return new Response(sseBody(frames.map((entry) => `${entry}\n\n`)), {
			status: 200,
			headers: { 'Content-Type': 'text/event-stream' }
		});
	}

	function routeFetch(scenario: {
		chat: Response;
		events: Response[];
		runs: Array<Record<string, unknown>>;
	}): string[] {
		const requested: string[] = [];
		const pendingEvents = [...scenario.events];
		const pendingRuns = [...scenario.runs];
		vi.stubGlobal(
			'fetch',
			vi.fn(async (input: unknown) => {
				const url = String(input);
				requested.push(url);
				if (url.endsWith('/assistant/chat')) return scenario.chat;
				if (url.includes('/events')) {
					const next = pendingEvents.shift();
					if (!next) throw new Error(`unexpected events request: ${url}`);
					return next;
				}
				if (url.includes(`/runs/${runId}`)) {
					const next = pendingRuns.shift();
					if (!next) throw new Error(`unexpected run request: ${url}`);
					return jsonResponse(next, 200);
				}
				throw new Error(`unexpected request: ${url}`);
			})
		);
		return requested;
	}

	async function streamDurable(
		onEvent: (event: AssistantChatStreamEvent) => void = () => {}
	): Promise<{ events: AssistantChatStreamEvent[]; run: unknown }> {
		const events: AssistantChatStreamEvent[] = [];
		const run = await streamAssistantChat(
			'project-1',
			{ conversation_id: 'c1', message: 'hi' },
			(event) => {
				events.push(event);
				onEvent(event);
			}
		);
		return { events, run };
	}

	it('streams a durable run through the existing frame renderer and returns it', async () => {
		const requested = routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [
				eventsResponse([
					frame('token', { delta: 'Hi' }),
					frame('tool_start', { tool: 'get_report', tool_call_id: 't1', args: {} }),
					frame('tool_complete', {
						tool: 'get_report',
						tool_call_id: 't1',
						output: { title: 'T' }
					}),
					frame('plan', { plan: { id: 'p1', status: 'pending', actions: [] } }),
					frame('done', { message_id: 'm1', input_tokens: 3, output_tokens: 5 })
				])
			],
			runs: [runJson({ status: 'completed', plan_id: 'p1' })]
		});

		const { events, run } = await streamDurable();

		expect(events).toEqual([
			{ event: 'token', delta: 'Hi' },
			{ event: 'tool_start', tool: 'get_report', tool_call_id: 't1', args: {} },
			{
				event: 'tool_complete',
				tool: 'get_report',
				tool_call_id: 't1',
				output: { title: 'T' }
			},
			{ event: 'plan', plan: { id: 'p1', status: 'pending', actions: [] } },
			{ event: 'done', message_id: 'm1', input_tokens: 3, output_tokens: 5 }
		]);
		expect(run).toMatchObject({ id: runId, status: 'completed', plan_id: 'p1' });
		expect(requested).toEqual([
			'/api/projects/project-1/assistant/chat',
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=-1`,
			`/api/projects/project-1/assistant/runs/${runId}`
		]);
	});

	it('resumes the events stream with after_seq while the run is still active', async () => {
		const requested = routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [
				eventsResponse([frame('token', { delta: 'Hel' })]),
				eventsResponse([
					frame('token', { delta: 'lo' }),
					frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 })
				])
			],
			runs: [runJson({ status: 'running' }), runJson({ status: 'completed' })]
		});

		const { events, run } = await streamDurable();

		expect(events.map((event) => event.event)).toEqual(['token', 'token', 'done']);
		expect(run).toMatchObject({ status: 'completed' });
		expect(requested).toEqual([
			'/api/projects/project-1/assistant/chat',
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=-1`,
			`/api/projects/project-1/assistant/runs/${runId}`,
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=0`,
			`/api/projects/project-1/assistant/runs/${runId}`
		]);
	});

	it('resumes from the server seq cursor when rows yield no frame', async () => {
		const requested = routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [
				// Seq 2 belongs to a row the server advances past without
				// emitting a frame (an unmapped kind). A frame count would
				// resume at after_seq=2 and replay seq 3; the server cursor
				// resumes past it.
				eventsResponse([
					frame('token', { delta: 'Hel' }, 0),
					frame('token', { delta: 'l' }, 1),
					frame('token', { delta: 'o' }, 3)
				]),
				eventsResponse([
					frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 }, 4)
				])
			],
			runs: [runJson({ status: 'running' }), runJson({ status: 'completed' })]
		});

		const { events, run } = await streamDurable();

		expect(events.map((event) => event.event)).toEqual(['token', 'token', 'token', 'done']);
		expect(run).toMatchObject({ status: 'completed' });
		expect(requested).toEqual([
			'/api/projects/project-1/assistant/chat',
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=-1`,
			`/api/projects/project-1/assistant/runs/${runId}`,
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=3`,
			`/api/projects/project-1/assistant/runs/${runId}`
		]);
	});

	it('returns the run plan_id when the plan frame arrives Null', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		try {
			routeFetch({
				chat: jsonResponse(runJson({ status: 'queued' }), 202),
				events: [
					eventsResponse([
						frame('token', { delta: 'Hi' }),
						frame('plan', { plan: null }),
						frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 })
					])
				],
				runs: [runJson({ status: 'completed', plan_id: 'plan-1' })]
			});

			const { events, run } = await streamDurable();

			expect(events.map((event) => event.event)).toEqual(['token', 'done']);
			// The UI falls back to this id to fetch and render the plan.
			expect(run).toMatchObject({ status: 'completed', plan_id: 'plan-1' });
			expect(warn).toHaveBeenCalled();
		} finally {
			warn.mockRestore();
		}
	});

	it('surfaces a stalled run instead of following it forever', async () => {
		const requested = routeFetch({
			chat: jsonResponse(runJson({ status: 'running' }), 202),
			events: [eventsResponse([]), eventsResponse([]), eventsResponse([])],
			runs: [
				runJson({ status: 'running' }),
				runJson({ status: 'running' }),
				runJson({ status: 'running' })
			]
		});

		const events: AssistantChatStreamEvent[] = [];
		const failure = await streamAssistantChat(
			'project-1',
			{ conversation_id: 'c1', message: 'hi' },
			(event) => events.push(event),
			undefined,
			{ maxStuckResumes: 3, sleep: async () => {} }
		).catch((caught: unknown) => caught);

		expect(failure).toBeInstanceOf(AssistantStreamError);
		expect(failure).toMatchObject({ status: 504, code: 'assistant_observe_stalled' });
		// A stall is only known as a rejection: no error frame is synthesized.
		expect(events).toEqual([]);
		expect(requested.filter((url) => url.includes('/events'))).toHaveLength(3);
	});

	it('times out a run that never settles without synthesizing an error frame', async () => {
		routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [eventsResponse([]), eventsResponse([]), eventsResponse([])],
			runs: [
				runJson({ status: 'running' }),
				runJson({ status: 'running' }),
				runJson({ status: 'running' })
			]
		});

		let now = 0;
		const events: AssistantChatStreamEvent[] = [];
		const failure = await streamAssistantChat(
			'project-1',
			{ conversation_id: 'c1', message: 'hi' },
			(event) => events.push(event),
			undefined,
			{
				timeoutMs: 1000,
				now: () => now,
				sleep: async () => {
					now += 600;
				}
			}
		).catch((caught: unknown) => caught);

		expect(failure).toBeInstanceOf(AssistantStreamError);
		expect(failure).toMatchObject({ status: 504, code: 'assistant_observe_timeout' });
		expect(events).toEqual([]);
	});

	it('reflects a failed run as an error event when no error frame arrived', async () => {
		routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [eventsResponse([frame('token', { delta: 'Hi' })])],
			runs: [
				runJson({
					status: 'failed',
					completed_at: '2026-01-01T00:00:03Z',
					error: {
						code: 'ai_budget_exceeded',
						message: 'AI budget for this month reached'
					}
				})
			]
		});

		const { events, run } = await streamDurable();

		expect(events).toEqual([
			{ event: 'token', delta: 'Hi' },
			{
				event: 'error',
				message: 'AI budget for this month reached',
				code: 'ai_budget_exceeded'
			}
		]);
		expect(run).toMatchObject({ status: 'failed' });
	});

	it('keeps the streamed error frame instead of duplicating the run error', async () => {
		routeFetch({
			chat: jsonResponse(runJson({ status: 'queued' }), 202),
			events: [eventsResponse([frame('error', { message: 'boom', code: 'x' })])],
			runs: [runJson({ status: 'failed', error: { code: 'x', message: 'boom' } })]
		});

		const { events } = await streamDurable();

		expect(events).toEqual([{ event: 'error', message: 'boom', code: 'x' }]);
	});

	it('rejects a 202 response without a run identity', async () => {
		vi.stubGlobal('fetch', vi.fn().mockResolvedValue(jsonResponse({ nope: true }, 202)));
		await expect(
			streamAssistantChat('project-1', { conversation_id: 'c1', message: 'hi' }, () => {})
		).rejects.toMatchObject({ status: 202 });
	});

	it('keeps the synchronous tool-command stream on a 200 response', async () => {
		const fetchMock = vi
			.fn()
			.mockResolvedValue(
				new Response(
					sseBody([
						`${frame('token', { delta: 'ok' })}\n\n`,
						`${frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 })}\n\n`
					])
				)
			);
		vi.stubGlobal('fetch', fetchMock);

		const events: AssistantChatStreamEvent[] = [];
		const run = await streamAssistantChat(
			'project-1',
			{ conversation_id: 'c1', message: '{"tool":"get_report"}' },
			(event) => events.push(event)
		);

		expect(events).toEqual([
			{ event: 'token', delta: 'ok' },
			{ event: 'done', message_id: 'm1', input_tokens: 1, output_tokens: 2 }
		]);
		expect(run).toBeNull();
		expect(fetchMock).toHaveBeenCalledTimes(1);
		expect(fetchMock.mock.calls[0][0]).toBe('/api/projects/project-1/assistant/chat');
	});

	it('reports resume progress from the run events stream', async () => {
		const fetchMock = vi
			.fn()
			.mockResolvedValue(
				eventsResponse([
					frame('token', { delta: 'Hi' }),
					frame('done', { message_id: 'm1', input_tokens: 1, output_tokens: 2 })
				])
			);
		vi.stubGlobal('fetch', fetchMock);

		const events: AssistantChatStreamEvent[] = [];
		const progress = await streamAssistantRunEvents(
			'project-1',
			runId,
			(event) => events.push(event),
			{ afterSeq: 4 }
		);

		expect(events.map((event) => event.event)).toEqual(['token', 'done']);
		expect(progress).toEqual({ frames: 2, nextAfterSeq: 6, terminal: true });
		expect(fetchMock.mock.calls[0][0]).toBe(
			`/api/projects/project-1/assistant/runs/${runId}/events?after_seq=4`
		);
	});

	it('honors a payload-embedded seq cursor without an SSE id', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValue(eventsResponse([frame('token', { delta: 'Hi', seq: 7 })]))
		);

		const events: AssistantChatStreamEvent[] = [];
		const progress = await streamAssistantRunEvents('project-1', runId, (event) =>
			events.push(event)
		);

		expect(events).toEqual([{ event: 'token', delta: 'Hi' }]);
		expect(progress).toEqual({ frames: 1, nextAfterSeq: 7, terminal: false });
	});

	it('warns loudly in dev when frames are dropped', async () => {
		const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
		try {
			vi.stubGlobal(
				'fetch',
				vi
					.fn()
					.mockResolvedValue(
						eventsResponse([frame('mystery', { x: 1 }), 'event: token\ndata: not json'])
					)
			);

			const events: AssistantChatStreamEvent[] = [];
			await streamAssistantRunEvents('project-1', runId, (event) => events.push(event));

			expect(events).toEqual([]);
			const warned = warn.mock.calls.map((call) => String(call[1]));
			expect(warned.some((payload) => payload.includes('mystery'))).toBe(true);
			expect(warned.some((payload) => payload.includes('not json'))).toBe(true);
		} finally {
			warn.mockRestore();
		}
	});

	it('raises run lookup failures as stream errors', async () => {
		vi.stubGlobal(
			'fetch',
			vi
				.fn()
				.mockResolvedValue(jsonResponse({ message: 'Run gone', code: 'run_missing' }, 404))
		);
		const error = await getAssistantRun('project-1', runId).catch((caught: unknown) => caught);
		expect(error).toBeInstanceOf(AssistantStreamError);
		expect(error).toMatchObject({ status: 404, message: 'Run gone', code: 'run_missing' });
	});
});
