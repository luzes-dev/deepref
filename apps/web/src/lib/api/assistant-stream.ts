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

/**
 * True in dev/test builds, false in production. Protocol drift is logged
 * loudly here while the parser stays tolerant everywhere.
 */
function isDev(): boolean {
	try {
		return import.meta.env?.DEV === true;
	} catch {
		return false;
	}
}

function devWarn(message: string, detail: unknown): void {
	if (isDev()) {
		console.warn(`[assistant-stream] ${message}`, detail);
	}
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
	return parseSseFrameInner(frame).event;
}

/**
 * Parses one SSE frame plus its server sequence cursor. The cursor travels
 * outside the JSON API shape: preferably the standard SSE `id:` field the
 * run-events endpoint sets per row, falling back to an integer `seq` member
 * of the event payload when present. Frames without either keep a null seq.
 */
function parseSseFrameInner(frame: string): {
	event: AssistantChatStreamEvent | null;
	seq: number | null;
	dropReason: string | null;
} {
	const { eventName, dataLine, frameId } = splitFrameLines(frame);
	const seq = frameId !== null && frameId !== '' ? parseSeqId(frameId) : null;

	if (!eventName || !dataLine) {
		return {
			event: null,
			seq,
			dropReason: !eventName ? 'frame has no event name' : 'frame has no data line'
		};
	}
	return decodeFrameBody(eventName, dataLine, seq);
}

/** The `event:` / `data:` / `id:` lines of one SSE frame. */
function splitFrameLines(frame: string): {
	eventName: string | null;
	dataLine: string | null;
	frameId: string | null;
} {
	let eventName: string | null = null;
	let dataLine: string | null = null;
	let frameId: string | null = null;

	for (const line of normalizeFrameBreaks(frame).split('\n')) {
		if (line.startsWith(':')) continue;
		if (line.startsWith('event:')) {
			eventName = line.slice('event:'.length).trim();
		} else if (line.startsWith('data:')) {
			dataLine = dataLine === null ? line.slice('data:'.length).trim() : dataLine;
		} else if (line.startsWith('id:')) {
			frameId = line.slice('id:'.length).trim();
		}
	}
	return { eventName, dataLine, frameId };
}

/** Builds the stream event for a validated `event:` + `data:` pair. */
function decodeFrameBody(
	eventName: string,
	dataLine: string,
	seq: number | null
): {
	event: AssistantChatStreamEvent | null;
	seq: number | null;
	dropReason: string | null;
} {
	const payload = safeJsonParse(dataLine);
	if (payload === null || typeof payload !== 'object') {
		return { event: null, seq, dropReason: 'frame data is not a JSON object' };
	}
	const record = payload as Payload;
	const cursor = seq ?? payloadSeq(record);

	if (!Object.hasOwn(STREAM_EVENTS, eventName)) {
		return { event: null, seq: cursor, dropReason: `unknown event '${eventName}'` };
	}
	const event = STREAM_EVENTS[eventName](record, eventName);
	if (!event) {
		return {
			event: null,
			seq: cursor,
			dropReason: `'${eventName}' frame carried no usable payload`
		};
	}
	return { event, seq: cursor, dropReason: null };
}

function parseSeqId(raw: string): number | null {
	return /^-?\d+$/.test(raw) ? Number(raw) : null;
}

function payloadSeq(payload: Payload): number | null {
	const seq = payload['seq'];
	return typeof seq === 'number' && Number.isInteger(seq) ? seq : null;
}

function safeJsonParse(raw: string): unknown {
	try {
		return JSON.parse(raw) as unknown;
	} catch {
		return null;
	}
}

type Payload = Record<string, unknown>;

function str(payload: Payload, key: string, eventName: string): string {
	const value = payload[key];
	if (typeof value === 'string') return value;
	devWarn(`'${eventName}' frame defaults string field '${key}' to ''`, payload);
	return '';
}

function num(payload: Payload, key: string, eventName: string): number {
	const value = payload[key];
	if (typeof value === 'number') return value;
	devWarn(`'${eventName}' frame defaults numeric field '${key}' to 0`, payload);
	return 0;
}

function argsRecord(payload: Payload, eventName: string): Record<string, unknown> {
	const value = payload['args'];
	if (value === undefined || value === null) return {};
	const record = objectOrNull(value);
	if (record) return record as Record<string, unknown>;
	devWarn(`'${eventName}' frame defaults non-object args to {}`, payload);
	return {};
}

function objectOrNull(value: unknown): object | null {
	return typeof value === 'object' && value !== null ? value : null;
}

const STREAM_EVENTS: Record<
	string,
	(payload: Payload, eventName: string) => AssistantChatStreamEvent | null
> = {
	token: (p, name) => ({ event: 'token', delta: str(p, 'delta', name) }),
	replace: (p, name) => ({ event: 'replace', text: str(p, 'text', name) }),
	tool_start: (p, name) => ({
		event: 'tool_start',
		tool: str(p, 'tool', name),
		tool_call_id: str(p, 'tool_call_id', name),
		args: argsRecord(p, name)
	}),
	tool_complete: (p, name) => ({
		event: 'tool_complete',
		tool: str(p, 'tool', name),
		tool_call_id: str(p, 'tool_call_id', name),
		output: p['output'] ?? null
	}),
	proposal_created: (p, name) => ({
		event: 'proposal_created',
		tool: str(p, 'tool', name),
		review_run_id: str(p, 'review_run_id', name),
		status_path: str(p, 'status_path', name)
	}),
	done: (p, name) => ({
		event: 'done',
		message_id: str(p, 'message_id', name),
		input_tokens: num(p, 'input_tokens', name),
		output_tokens: num(p, 'output_tokens', name)
	}),
	status: (p, name) => ({ event: 'status', message: str(p, 'message', name) }),
	plan: (p) => (objectOrNull(p['plan']) ? { event: 'plan', plan: p['plan'] } : null),
	error: (p, name) => ({
		event: 'error',
		message: str(p, 'message', name) || 'assistant turn failed',
		code: str(p, 'code', name) || null
	})
};

async function consumeSseStream(
	body: ReadableStream<Uint8Array>,
	onEvent: (event: AssistantChatStreamEvent) => void,
	onSeq?: (seq: number | null) => void
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
			consumeFrame(frame, onEvent, onSeq);
		}
	}

	buffer += decoder.decode();
	const { frames } = splitSseFrames(`${buffer}\n\n`);
	for (const frame of frames) {
		consumeFrame(frame, onEvent, onSeq);
	}
}

/**
 * Delivers one complete frame: parsed events go to the renderer, dropped
 * frames are reported loudly in dev with the raw payload, and every frame's
 * server seq (when provided) advances the resume cursor — including dropped
 * frames, so a resume never replays them.
 */
function consumeFrame(
	frame: string,
	onEvent: (event: AssistantChatStreamEvent) => void,
	onSeq?: (seq: number | null) => void
): void {
	const parsed = parseSseFrameInner(frame);
	if (parsed.event) {
		onEvent(parsed.event);
	} else if (parsed.dropReason) {
		devWarn(`dropping assistant stream frame (${parsed.dropReason})`, frame);
	}
	onSeq?.(parsed.seq);
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
	/**
	 * The after_seq value that resumes right after the last row seen: the
	 * highest server seq observed on this stream. Rows the server advances
	 * past without emitting a frame (unmapped kinds) and frames dropped
	 * client-side are already covered, so resuming here neither skips nor
	 * replays. Only when the server sent no seq at all does this fall back
	 * to the legacy frame count (afterSeq + frames).
	 */
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
	let maxSeq: number | null = null;
	await consumeSseStream(
		response.body,
		(event) => {
			frames += 1;
			if (event.event === 'done' || event.event === 'error') terminal = true;
			onEvent(event);
		},
		(seq) => {
			if (seq !== null) maxSeq = maxSeq === null ? seq : Math.max(maxSeq, seq);
		}
	);
	const nextAfterSeq = maxSeq === null ? afterSeq + frames : Math.max(afterSeq, maxSeq);
	return { frames, nextAfterSeq, terminal };
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
 * Bounds for following a durable run. Observation is best-effort: the worker
 * owns execution, so giving up here never cancels the run — the caller
 * surfaces the rejection (e.g. offers a retry) while the run continues.
 */
export interface FollowAssistantRunOptions {
	signal?: AbortSignal;
	/** Wall-clock budget for the whole follow, in ms. Default 10 minutes. */
	timeoutMs?: number;
	/** Maximum events-stream rounds before giving up. Default 50. */
	maxResumes?: number;
	/**
	 * Consecutive rounds with no new frames and no status change before the
	 * run is treated as stalled. Default 10.
	 */
	maxStuckResumes?: number;
	/** First reconnect delay in ms; doubles per stuck round. Default 500. */
	baseDelayMs?: number;
	/** Cap for the reconnect delay in ms. Default 8000. */
	maxDelayMs?: number;
	/** Injectable for tests. Waits ms unless the signal aborts. */
	sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
	/** Injectable clock for tests. */
	now?: () => number;
}

const FOLLOW_TIMEOUT_MS = 10 * 60_000;
const FOLLOW_MAX_RESUMES = 50;
const FOLLOW_MAX_STUCK_RESUMES = 10;
const FOLLOW_BASE_DELAY_MS = 500;
const FOLLOW_MAX_DELAY_MS = 8_000;

function abortableSleep(ms: number, signal?: AbortSignal): Promise<void> {
	signal?.throwIfAborted();
	if (ms <= 0) return Promise.resolve();
	return new Promise<void>((resolve, reject) => {
		const timer = setTimeout(() => {
			signal?.removeEventListener('abort', onAbort);
			resolve();
		}, ms);
		const onAbort = (): void => {
			clearTimeout(timer);
			try {
				signal?.throwIfAborted();
				resolve();
			} catch (error) {
				reject(error);
			}
		};
		signal?.addEventListener('abort', onAbort, { once: true });
	});
}

/**
 * Follows a 202 run to settlement: streams persisted events through the
 * existing frame renderer, then reflects the run record for its terminal
 * status, plan link and errors. When the events stream closes while the run
 * is still active (a transient disconnect), it backs off and resumes with
 * the server seq cursor.
 *
 * The loop is bounded three ways: an overall timeout, a cap on reconnect
 * rounds, and stuck detection (no frames and no status change across
 * consecutive rounds). Each surfaces a rejection — never a synthesized
 * error frame. Error frames are synthesized only where a terminal failure
 * is actually known: a failed/cancelled run record with no error frame.
 */
async function followAssistantRun(
	projectId: string,
	initialRun: AssistantRunDto,
	onEvent: (event: AssistantChatStreamEvent) => void,
	options?: FollowAssistantRunOptions
): Promise<AssistantRunDto> {
	const bounds = resolveFollowBounds(options);
	const cursor: FollowCursor = {
		run: initialRun,
		afterSeq: -1,
		sawTerminalFrame: false,
		resumes: 0,
		stuckResumes: 0,
		lastStatus: initialRun.status,
		startedAt: bounds.now()
	};

	for (;;) {
		bounds.signal?.throwIfAborted();
		throwIfFollowTimedOut(cursor, bounds);
		const progress = await streamAssistantRunEvents(projectId, cursor.run.id, onEvent, {
			afterSeq: cursor.afterSeq,
			signal: bounds.signal
		});
		cursor.resumes += 1;
		throwIfResumesExhausted(cursor, bounds);
		cursor.afterSeq = progress.nextAfterSeq;
		cursor.sawTerminalFrame = cursor.sawTerminalFrame || progress.terminal;

		cursor.run = await getAssistantRun(projectId, cursor.run.id, bounds.signal);
		const progressed = progress.frames > 0 || cursor.run.status !== cursor.lastStatus;
		cursor.lastStatus = cursor.run.status;
		const settled = settleFollowedRun(cursor.run, cursor.sawTerminalFrame, onEvent);
		if (settled) return settled;
		await settleFollowRound(progressed, cursor, bounds);
	}
}

/** One follow loop's worth of mutable progress. */
interface FollowCursor {
	run: AssistantRunDto;
	afterSeq: number;
	sawTerminalFrame: boolean;
	resumes: number;
	stuckResumes: number;
	lastStatus: unknown;
	startedAt: number;
}

/** Follow bounds with defaults applied; resolved once per follow. */
type ResolvedFollowBounds = Omit<Required<FollowAssistantRunOptions>, 'signal'> & {
	signal?: AbortSignal;
};

function resolveFollowBounds(options?: FollowAssistantRunOptions): ResolvedFollowBounds {
	const {
		signal,
		timeoutMs = FOLLOW_TIMEOUT_MS,
		maxResumes = FOLLOW_MAX_RESUMES,
		maxStuckResumes = FOLLOW_MAX_STUCK_RESUMES,
		baseDelayMs = FOLLOW_BASE_DELAY_MS,
		maxDelayMs = FOLLOW_MAX_DELAY_MS,
		sleep = abortableSleep,
		now = Date.now
	} = options ?? {};
	return { signal, timeoutMs, maxResumes, maxStuckResumes, baseDelayMs, maxDelayMs, sleep, now };
}

function throwObserveTimeout(): never {
	throw new AssistantStreamError(
		504,
		'The assistant is taking too long to answer. It keeps working in the background; check back shortly.',
		'assistant_observe_timeout'
	);
}

function throwObserveStalled(): never {
	throw new AssistantStreamError(
		504,
		'The assistant stopped sending updates. The run may still be working; check back shortly.',
		'assistant_observe_stalled'
	);
}

function throwIfFollowTimedOut(cursor: FollowCursor, bounds: ResolvedFollowBounds): void {
	if (bounds.now() - cursor.startedAt > bounds.timeoutMs) throwObserveTimeout();
}

function throwIfResumesExhausted(cursor: FollowCursor, bounds: ResolvedFollowBounds): void {
	if (cursor.resumes > bounds.maxResumes) throwObserveTimeout();
}

/**
 * Returns the run when it reached a terminal status (synthesizing the error
 * frame for failed/cancelled runs that never streamed one), else null.
 */
function settleFollowedRun(
	run: AssistantRunDto,
	sawTerminalFrame: boolean,
	onEvent: (event: AssistantChatStreamEvent) => void
): AssistantRunDto | null {
	if (!isTerminalRunStatus(run.status)) return null;
	if ((run.status === 'failed' || run.status === 'cancelled') && !sawTerminalFrame) {
		onEvent(runFailureEvent(run));
	}
	return run;
}

/** Records one round's progress: resets the stuck counter, or backs off, or throws stalled. */
async function settleFollowRound(
	progressed: boolean,
	cursor: FollowCursor,
	bounds: ResolvedFollowBounds
): Promise<void> {
	if (progressed) {
		cursor.stuckResumes = 0;
		return;
	}
	cursor.stuckResumes += 1;
	if (cursor.stuckResumes >= bounds.maxStuckResumes) throwObserveStalled();
	const delay = Math.min(bounds.maxDelayMs, bounds.baseDelayMs * 2 ** (cursor.stuckResumes - 1));
	await bounds.sleep(delay, bounds.signal);
}

export async function streamAssistantChat(
	projectId: string,
	body: { conversation_id: string; message: string },
	onEvent: (event: AssistantChatStreamEvent) => void,
	signal?: AbortSignal,
	follow?: FollowAssistantRunOptions
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
		return followAssistantRun(projectId, run, onEvent, {
			...follow,
			signal: signal ?? follow?.signal
		});
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
	const value = parsed?.[key];
	const text = typeof value === 'string' ? value : '';
	return text.length > 0 ? text : undefined;
}
