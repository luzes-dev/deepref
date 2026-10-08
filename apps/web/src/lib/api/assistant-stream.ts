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

	return normalizeStreamEvent(eventName, payload as Record<string, unknown>);
}

function safeJsonParse(raw: string): unknown {
	try {
		return JSON.parse(raw) as unknown;
	} catch {
		return null;
	}
}

function normalizeStreamEvent(
	eventName: string,
	payload: Record<string, unknown>
): AssistantChatStreamEvent | null {
	const str = (key: string): string =>
		typeof payload[key] === 'string' ? (payload[key] as string) : '';
	const num = (key: string): number =>
		typeof payload[key] === 'number' ? (payload[key] as number) : 0;

	switch (eventName) {
		case 'token':
			return { event: 'token', delta: str('delta') };
		case 'replace':
			return { event: 'replace', text: str('text') };
		case 'tool_start':
			return {
				event: 'tool_start',
				tool: str('tool'),
				tool_call_id: str('tool_call_id'),
				args:
					typeof payload['args'] === 'object' && payload['args'] !== null
						? (payload['args'] as Record<string, unknown>)
						: {}
			};
		case 'tool_complete':
			return {
				event: 'tool_complete',
				tool: str('tool'),
				tool_call_id: str('tool_call_id'),
				output: payload['output'] ?? null
			};
		case 'proposal_created':
			return {
				event: 'proposal_created',
				tool: str('tool'),
				review_run_id: str('review_run_id'),
				status_path: str('status_path')
			};
		case 'done':
			return {
				event: 'done',
				message_id: str('message_id'),
				input_tokens: num('input_tokens'),
				output_tokens: num('output_tokens')
			};
		case 'status':
			return { event: 'status', message: str('message') };
		case 'plan':
			return payload['plan'] && typeof payload['plan'] === 'object'
				? { event: 'plan', plan: payload['plan'] }
				: null;
		case 'error':
			return {
				event: 'error',
				message: str('message') || 'assistant turn failed',
				code: str('code') || null
			};
		default:
			return null;
	}
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
			'x-actor-id': 'local-user'
		},
		body: JSON.stringify(body),
		signal
	});

	if (!response.ok) {
		const detail = await response.text().catch(() => '');
		const message = extractErrorMessage(detail) ?? response.statusText ?? 'chat request failed';
		throw new AssistantStreamError(response.status, message, extractErrorCode(detail));
	}

	if (!response.body) {
		throw new AssistantStreamError(response.status, 'assistant stream is unavailable');
	}

	await consumeSseStream(response.body, onEvent);
}

function extractErrorCode(raw: string): string | null {
	const parsed = safeJsonParse(raw);
	if (typeof parsed === 'object' && parsed !== null && 'code' in parsed) {
		const code = (parsed as Record<string, unknown>)['code'];
		if (typeof code === 'string' && code.length > 0) return code;
	}
	return null;
}

function extractErrorMessage(raw: string): string | undefined {
	if (!raw.trim()) return undefined;
	const parsed = safeJsonParse(raw);
	if (typeof parsed === 'object' && parsed !== null && 'message' in parsed) {
		const message = (parsed as Record<string, unknown>)['message'];
		if (typeof message === 'string' && message.length > 0) return message;
	}
	return undefined;
}
