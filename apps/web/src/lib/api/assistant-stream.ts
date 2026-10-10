import type { AssistantRunDto } from './generated/models/index.js';
import { currentReviewerId } from './reviewer';

export type AssistantChatStreamEvent =
	| { event: 'token'; delta: string }
	| { event: 'replace'; text: string }
	| { event: 'status'; message: string }
	| { event: 'plan'; plan: unknown }
	| {
			event: 'tool_start';
			tool: string;
			tool_call_id: string;
			args: Record<string, unknown>;
	  }
	| {
			event: 'tool_complete';
			tool: string;
			tool_call_id: string;
			output: unknown;
	  }
	| {
			event: 'proposal_created';
			tool: string;
			review_run_id: string;
			status_path: string;
	  }
	| {
			event: 'done';
			message_id: string;
			input_tokens: number;
			output_tokens: number;
	  }
	| { event: 'error'; message: string; code: string | null };

export class AssistantStreamError extends Error {
	readonly status: number;
	readonly code: string | null;

	constructor(status: number, message: string, code: string | null = null) {
		super(message);
		this.name = 'AssistantStreamError';
		this.status = status;
		this.code = code;
	}
}

function normalizeFrameBreaks(raw: string): string {
	return raw.replaceAll('\r\n', '\n').replaceAll('\r', '\n');
}

export function splitSseFrames(raw: string): {
	frames: string[];
	remainder: string;
} {
	const normalized = normalizeFrameBreaks(raw);
	const parts = normalized.split('\n\n');
	const remainder = parts.pop() ?? '';
	return { frames: parts.filter((frame) => frame.trim().length > 0), remainder };
}

export function parseSseFrame(frame: string): AssistantChatStreamEvent | null {
	let eventName: string | null = null;
	let dataLine: string | null = null;

	for (const line of normalizeFrameBreaks(frame).split('\n')) {
		if (line.startsWith(':')) continue;
		if (line.startsWith('event:')) {
			eventName = line.slice('event:'.length).trim();
		} else if (line.startsWith('data:')) {
			dataLine = dataLine === null ? line.slice('data:'.length).trim() : dataLine;
		}
	}

	if (!eventName || !dataLine) return null;

	const payload = safeJsonParse(dataLine);
	if (payload === null || typeof payload !== 'object') return null;

	return normalizeStreamEvent(eventName, payload as Payload);
}

function safeJsonParse(raw: string): unknown {
	try {
		return JSON.parse(raw) as unknown;
	} catch {
		return null;
	}
}

type Payload = Record<string, unknown>;

function str(payload: Payload, key: string): string {
	const value = payload[key];
	return typeof value === 'string' ? value : '';
}

function num(payload: Payload, key: string): number {
	const value = payload[key];
	return typeof value === 'number' ? value : 0;
}

function objectOrNull(value: unknown): object | null {
	return typeof value === 'object' && value !== null ? value : null;
}

const STREAM_EVENTS: Record<string, (payload: Payload) => AssistantChatStreamEvent | null> = {
	token: (p) => ({ event: 'token', delta: str(p, 'delta') }),
	replace: (p) => ({ event: 'replace', text: str(p, 'text') }),
	tool_start: (p) => ({
		event: 'tool_start',
		tool: str(p, 'tool'),
		tool_call_id: str(p, 'tool_call_id'),
		args: (objectOrNull(p['args']) ?? {}) as Record<string, unknown>
	}),
	tool_complete: (p) => ({
		event: 'tool_complete',
		tool: str(p, 'tool'),
		tool_call_id: str(p, 'tool_call_id'),
		output: p['output'] ?? null
	}),
	proposal_created: (p) => ({
		event: 'proposal_created',
		tool: str(p, 'tool'),
		review_run_id: str(p, 'review_run_id'),
		status_path: str(p, 'status_path')
	}),
	done: (p) => ({
		event: 'done',
		message_id: str(p, 'message_id'),
		input_tokens: num(p, 'input_tokens'),
		output_tokens: num(p, 'output_tokens')
	}),
	status: (p) => ({ event: 'status', message: str(p, 'message') }),
	plan: (p) => (objectOrNull(p['plan']) ? { event: 'plan', plan: p['plan'] } : null),
	error: (p) => ({
		event: 'error',
		message: str(p, 'message') || 'assistant turn failed',
		code: str(p, 'code') || null
	})
};

function normalizeStreamEvent(
	eventName: string,
	payload: Payload
): AssistantChatStreamEvent | null {
	const build = Object.hasOwn(STREAM_EVENTS, eventName) ? STREAM_EVENTS[eventName] : undefined;
	return build ? build(payload) : null;
}

async function consumeSseStream(
	body: ReadableStream<Uint8Array>,
	onEvent: (event: AssistantChatStreamEvent) => void
): Promise<void> {
	const reader = body.getReader();
	const decoder = new TextDecoder();
	let buffer = '';

	for (;;) {
		const { done, value } = await reader.read();
		if (done) break;
		buffer += decoder.decode(value, { stream: true });
		const { frames, remainder } = splitSseFrames(buffer);
		buffer = remainder;
		for (const frame of frames) {
			const event = parseSseFrame(frame);
			if (event) onEvent(event);
		}
	}

	buffer += decoder.decode();
	const { frames } = splitSseFrames(`${buffer}\n\n`);
	for (const frame of frames) {
		const event = parseSseFrame(frame);
		if (event) onEvent(event);
	}
}

const TERMINAL_RUN_STATUSES = new Set(['completed', 'failed', 'cancelled']);

function isTerminalRunStatus(status: unknown): boolean {
	return typeof status === 'string' && TERMINAL_RUN_STATUSES.has(status);
}

function runEventsUrl(projectId: string, runId: string, afterSeq: number): string {
	return `/api/projects/${projectId}/assistant/runs/${runId}/events?after_seq=${afterSeq}`;
}

function runUrl(projectId: string, runId: string): string {
	return `/api/projects/${projectId}/assistant/runs/${runId}`;
}

function actorHeaders(): Record<string, string> {
	return {
		'x-actor-kind': 'user',
		'x-actor-id': currentReviewerId()
	};
}

async function throwForRunResponse(response: Response): Promise<never> {
	const detail = await response.text().catch(() => '');
	const message =
		errorField(detail, 'message') ?? response.statusText ?? 'assistant run request failed';
	throw new AssistantStreamError(response.status, message, errorField(detail, 'code') ?? null);
}

async function readAssistantRun(response: Response): Promise<AssistantRunDto> {
	let body: unknown;
	try {
		body = (await response.json()) as unknown;
	} catch {
		throw new AssistantStreamError(response.status, 'assistant run response was not JSON');
	}
	if (typeof body !== 'object' || body === null) {
		throw new AssistantStreamError(response.status, 'assistant run response was malformed');
	}
	const run = body as Record<string, unknown>;
	if (typeof run['id'] !== 'string') {
		throw new AssistantStreamError(response.status, 'assistant run response was malformed');
	}
	return body as AssistantRunDto;
}

/** Reflects a terminal failed/cancelled run as the error frame the UI already renders. */
function runFailureEvent(run: AssistantRunDto): AssistantChatStreamEvent {
	const error =
		typeof run.error === 'object' && run.error !== null
			? (run.error as Record<string, unknown>)
			: {};
	const message =
		typeof error['message'] === 'string' && error['message'].length > 0
			? error['message']
			: 'assistant turn failed';
	const code =
		typeof error['code'] === 'string' && error['code'].length > 0 ? error['code'] : null;
	return { event: 'error', message, code };
}

export interface AssistantRunEventsProgress {
	/** Frames delivered to onEvent by this stream. */
	frames: number;
	/** The after_seq value that resumes right after the last frame seen. */
	nextAfterSeq: number;
	/** Whether a done or error frame arrived on this stream. */
	terminal: boolean;
}

/**
 * Observes one durable run over its persisted events. Frame rendering is the
 * same SSE parser the synchronous tool-command path uses. Pass the returned
 * nextAfterSeq back as afterSeq to resume after a disconnect; the worker owns
 * execution, so observing again never restarts the turn.
 */
export async function streamAssistantRunEvents(
	projectId: string,
	runId: string,
	onEvent: (event: AssistantChatStreamEvent) => void,
	options?: { afterSeq?: number; signal?: AbortSignal }
): Promise<AssistantRunEventsProgress> {
	const afterSeq = options?.afterSeq ?? -1;
	// fallow-ignore-next-line security-sink -- Fixed-origin internal API proxy endpoint
	const response = await fetch(runEventsUrl(projectId, runId, afterSeq), {
		headers: actorHeaders(),
		signal: options?.signal
	});

	if (!response.ok) {
		await throwForRunResponse(response);
	}

	if (!response.body) {
		throw new AssistantStreamError(response.status, 'assistant run events are unavailable');
	}

	let frames = 0;
	let terminal = false;
	await consumeSseStream(response.body, (event) => {
		frames += 1;
		if (event.event === 'done' || event.event === 'error') terminal = true;
		onEvent(event);
	});
	return { frames, nextAfterSeq: afterSeq + frames, terminal };
}

/** Reads the durable run identity the 202 chat response carries. */
export async function getAssistantRun(
	projectId: string,
	runId: string,
	signal?: AbortSignal
): Promise<AssistantRunDto> {
	// fallow-ignore-next-line security-sink -- Fixed-origin internal API proxy endpoint
	const response = await fetch(runUrl(projectId, runId), {
		headers: actorHeaders(),
		signal
	});

	if (!response.ok) {
		await throwForRunResponse(response);
	}

	return readAssistantRun(response);
}

/**
 * Follows a 202 run to settlement: streams persisted events through the
 * existing frame renderer, then reflects the run record for its terminal
 * status, plan link and errors. When the events stream closes while the run
 * is still active (a transient disconnect), it resumes with after_seq.
 */
async function followAssistantRun(
	projectId: string,
	initialRun: AssistantRunDto,
	onEvent: (event: AssistantChatStreamEvent) => void,
	signal?: AbortSignal
): Promise<AssistantRunDto> {
	let run = initialRun;
	let afterSeq = -1;
	let sawTerminalFrame = false;

	for (;;) {
		const progress = await streamAssistantRunEvents(projectId, run.id, onEvent, {
			afterSeq,
			signal
		});
		afterSeq = progress.nextAfterSeq;
		sawTerminalFrame = sawTerminalFrame || progress.terminal;

		run = await getAssistantRun(projectId, run.id, signal);
		if (isTerminalRunStatus(run.status)) {
			if ((run.status === 'failed' || run.status === 'cancelled') && !sawTerminalFrame) {
				onEvent(runFailureEvent(run));
			}
			return run;
		}
	}
}

export async function streamAssistantChat(
	projectId: string,
	body: { conversation_id: string; message: string },
	onEvent: (event: AssistantChatStreamEvent) => void,
	signal?: AbortSignal
): Promise<AssistantRunDto | null> {
	// fallow-ignore-next-line security-sink -- Fixed-origin internal API proxy endpoint
	const response = await fetch(`/api/projects/${projectId}/assistant/chat`, {
		method: 'POST',
		headers: {
			'Content-Type': 'application/json',
			'x-actor-kind': 'user',
			'x-actor-id': currentReviewerId()
		},
		body: JSON.stringify(body),
		signal
	});

	if (!response.ok) {
		const detail = await response.text().catch(() => '');
		const message =
			errorField(detail, 'message') ?? response.statusText ?? 'chat request failed';
		throw new AssistantStreamError(
			response.status,
			message,
			errorField(detail, 'code') ?? null
		);
	}

	if (response.status === 202) {
		const run = await readAssistantRun(response);
		return followAssistantRun(projectId, run, onEvent, signal);
	}

	if (!response.body) {
		throw new AssistantStreamError(response.status, 'assistant stream is unavailable');
	}

	await consumeSseStream(response.body, onEvent);
	return null;
}

/** A non-empty string field of a JSON error body, if the body is JSON and has one. */
function errorField(raw: string, key: 'code' | 'message'): string | undefined {
	const parsed = objectOrNull(safeJsonParse(raw)) as Payload | null;
	const value = parsed ? str(parsed, key) : '';
	return value.length > 0 ? value : undefined;
}
