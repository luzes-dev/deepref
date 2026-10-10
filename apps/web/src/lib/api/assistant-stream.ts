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

export async function streamAssistantChat(
	projectId: string,
	body: { conversation_id: string; message: string },
	onEvent: (event: AssistantChatStreamEvent) => void,
	signal?: AbortSignal
): Promise<void> {
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

	if (!response.body) {
		throw new AssistantStreamError(response.status, 'assistant stream is unavailable');
	}

	await consumeSseStream(response.body, onEvent);
}

/** A non-empty string field of a JSON error body, if the body is JSON and has one. */
function errorField(raw: string, key: 'code' | 'message'): string | undefined {
	const parsed = objectOrNull(safeJsonParse(raw)) as Payload | null;
	const value = parsed ? str(parsed, key) : '';
	return value.length > 0 ? value : undefined;
}
