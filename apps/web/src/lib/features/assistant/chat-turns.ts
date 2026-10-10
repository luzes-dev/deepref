import type { AssistantChatStreamEvent } from '#lib/api/assistant-stream.js';
import { isAssistantPlan, type AssistantMessageRecord, type AssistantPlan } from './chat-api';

/** The sentence the server stores after a stopped answer. Keep the two in step. */
export const STOPPED_NOTE = 'Stopped. Nothing has been changed.';

const MAX_CITATIONS = 8;
const CITATIONS_PER_TOOL = 4;
const LABEL_CHARS = 40;

export interface ToolCallView {
	toolCallId: string;
	tool: string;
	args: Record<string, unknown> | null;
	status: 'running' | 'completed' | 'failed';
	output: unknown;
}

export interface ProposalView {
	tool: string;
	reviewRunId: string;
}

/** A record the answer relied on. The id is kept for the link and the tooltip only. */
export interface CitationView {
	key: string;
	label: string;
	tooltip: string;
	href: string | null;
}

export interface AssistantTurn {
	kind: 'assistant';
	id: string;
	content: string;
	createdAt: string | null;
	tools: ToolCallView[];
	proposals: ProposalView[];
	citations: CitationView[];
	tokens: { input: number; output: number } | null;
	planId: string | null;
	plan: AssistantPlan | null;
	planBusy: boolean;
	planError: string | null;
	stopped: boolean;
}

export interface UserTurn {
	kind: 'user';
	id: string;
	content: string;
	createdAt: string | null;
}

export type ChatTurn = UserTurn | AssistantTurn;

export function emptyAssistantTurn(id: string): AssistantTurn {
	return {
		kind: 'assistant',
		id,
		content: '',
		createdAt: null,
		tools: [],
		proposals: [],
		citations: [],
		tokens: null,
		planId: null,
		plan: null,
		planBusy: false,
		planError: null,
		stopped: false
	};
}

/**
 * Applies one stream event. The answer text restarts when a tool starts, the
 * same rule the server follows when it stores the reply, so the bubble and the
 * stored message agree.
 */
export function reduceAssistantTurn(
	turn: AssistantTurn,
	event: AssistantChatStreamEvent,
	projectId: string
): AssistantTurn {
	switch (event.event) {
		case 'token':
			return { ...turn, content: turn.content + event.delta };
		case 'replace':
			return { ...turn, content: event.text };
		case 'tool_start':
			return {
				...turn,
				content: '',
				tools: [
					...turn.tools,
					{
						toolCallId: event.tool_call_id,
						tool: event.tool,
						args: event.args,
						status: 'running',
						output: null
					}
				]
			};
		case 'tool_complete':
			return {
				...turn,
				tools: turn.tools.map((call): ToolCallView =>
					call.toolCallId === event.tool_call_id
						? { ...call, status: 'completed', output: event.output }
						: call
				),
				citations: mergeCitations(
					turn.citations,
					harvestCitations(event.tool, event.output, projectId)
				)
			};
		case 'proposal_created':
			return {
				...turn,
				proposals: [
					...turn.proposals,
					{ tool: event.tool, reviewRunId: event.review_run_id }
				]
			};
		case 'plan':
			return isAssistantPlan(event.plan)
				? { ...turn, plan: event.plan, planId: event.plan.id }
				: turn;
		case 'done':
			return {
				...turn,
				tokens: { input: event.input_tokens, output: event.output_tokens }
			};
		case 'status':
		case 'error':
			return turn;
	}
}

/**
 * The user pressed Stop. Keeps the text already shown, adds the same note the
 * server stores, and marks the tools that never finished as failed.
 */
export function markStopped(turn: AssistantTurn): AssistantTurn {
	const text = turn.content.trimEnd();
	return {
		...turn,
		content: text ? `${text}\n\n${STOPPED_NOTE}` : STOPPED_NOTE,
		stopped: true,
		tools: turn.tools.map((call): ToolCallView =>
			call.status === 'running' ? { ...call, status: 'failed' } : call
		)
	};
}

type CitationKind = 'protocol' | 'passage' | 'report' | 'study' | 'record';
type RecordFields = Record<string, unknown> & { id: string };

function citationKind(tool: string): CitationKind {
	if (tool === 'get_project_protocol') return 'protocol';
	if (tool.startsWith('read_document') || tool.startsWith('search_document')) return 'passage';
	if (tool.includes('report')) return 'report';
	if (tool.includes('study')) return 'study';
	return 'record';
}

function visitRecords(value: unknown, visit: (record: RecordFields) => void): void {
	if (Array.isArray(value)) {
		for (const item of value) visitRecords(item, visit);
		return;
	}
	if (typeof value !== 'object' || value === null) return;
	const record = value as Record<string, unknown>;
	if (typeof record['id'] === 'string') visit(record as RecordFields);
	for (const nested of Object.values(record)) {
		if (typeof nested === 'object' && nested !== null) visitRecords(nested, visit);
	}
}

const ENTITY_TEXT: Record<string, string> = {
	amp: '&',
	lt: '<',
	gt: '>',
	quot: '"',
	apos: "'",
	'#39': "'"
};

/** Titles arrive with HTML entities from the imports, e.g. `&amp;`. */
export function decodeEntities(value: string): string {
	return value.replace(
		/&(#39|amp|lt|gt|quot|apos);/g,
		(match, name: string) => ENTITY_TEXT[name] ?? match
	);
}

function shorten(value: string, max: number): string {
	const clean = value.replace(/\s+/g, ' ').trim();
	if (clean.length <= max) return clean;
	const cut = clean.slice(0, max);
	const boundary = cut.lastIndexOf(' ');
	return `${(boundary > max / 2 ? cut.slice(0, boundary) : cut).trimEnd()}…`;
}

function titleOf(record: RecordFields): string | null {
	const raw = record['title'] ?? record['name'];
	return typeof raw === 'string' && raw.trim() ? decodeEntities(raw.trim()) : null;
}

function labelFor(kind: CitationKind, record: RecordFields, title: string | null): string {
	switch (kind) {
		case 'passage': {
			const page = record['page_number'];
			return typeof page === 'number' ? `Passage, page ${String(page)}` : 'Passage';
		}
		case 'report': {
			const short = shorten(title ?? 'Report', LABEL_CHARS);
			const year = record['publication_year'];
			return typeof year === 'number' ? `${String(year)} · ${short}` : short;
		}
		case 'protocol':
			return shorten(title ?? 'Protocol', LABEL_CHARS);
		case 'study':
			return shorten(title ?? 'Study', LABEL_CHARS);
		case 'record':
			return shorten(title ?? 'Record', LABEL_CHARS);
	}
}

/** Only places the app has a page for get a link: a report, a study, the protocol. */
function hrefFor(kind: CitationKind, id: string, projectId: string): string | null {
	const encoded = encodeURIComponent(id);
	switch (kind) {
		case 'report':
			return `/projects/${projectId}/articles?report=${encoded}`;
		case 'study':
			return `/projects/${projectId}/studies?study=${encoded}`;
		case 'protocol':
			return `/projects/${projectId}/protocol`;
		case 'passage':
		case 'record':
			return null;
	}
}

/** Human-readable citations for the records in one tool result. */
export function harvestCitations(tool: string, output: unknown, projectId: string): CitationView[] {
	const kind = citationKind(tool);
	const found = new Map<string, CitationView>();
	visitRecords(output, (record) => {
		if (found.has(record.id)) return;
		const title = titleOf(record);
		found.set(record.id, {
			key: record.id,
			label: labelFor(kind, record, title),
			tooltip: `${title ?? labelFor(kind, record, null)} · ${record.id}`,
			href: hrefFor(kind, record.id, projectId)
		});
	});
	return [...found.values()].slice(0, CITATIONS_PER_TOOL);
}

function mergeCitations(existing: CitationView[], incoming: CitationView[]): CitationView[] {
	const merged = new Map<string, CitationView>();
	for (const citation of [...existing, ...incoming]) {
		if (!merged.has(citation.key)) merged.set(citation.key, citation);
	}
	return [...merged.values()].slice(0, MAX_CITATIONS);
}

function readTokenCounts(
	metadata: Record<string, unknown> | null
): { input: number; output: number } | null {
	if (!metadata) return null;
	const input = metadata['input_tokens'];
	const output = metadata['output_tokens'];
	if (typeof input !== 'number' || typeof output !== 'number') return null;
	return { input, output };
}

function readPlanId(metadata: Record<string, unknown> | null): string | null {
	const value = metadata?.['plan_id'];
	return typeof value === 'string' ? value : null;
}

/** Maps a stored message to the turn the thread shows. */
export function recordToTurn(record: AssistantMessageRecord, projectId: string): ChatTurn | null {
	if (record.role === 'user') {
		return {
			kind: 'user',
			id: record.id,
			content: record.content,
			createdAt: record.created_at
		};
	}
	if (record.role !== 'assistant') return null;

	const results = record.tool_results ?? [];
	const tools: ToolCallView[] = (record.tool_calls ?? []).map((call): ToolCallView => {
		const result = results.find((candidate) => candidate.tool_call_id === call.id);
		return {
			toolCallId: call.id,
			tool: call.tool,
			args: call.args,
			status: result ? 'completed' : 'failed',
			output: result?.output ?? null
		};
	});
	const proposals: ProposalView[] = results
		.filter((result) => typeof result.proposal_review_run_id === 'string')
		.map((result) => ({
			tool: result.tool,
			reviewRunId: result.proposal_review_run_id as string
		}));
	const citations = results.reduce<CitationView[]>(
		(all, result) =>
			mergeCitations(all, harvestCitations(result.tool, result.output, projectId)),
		[]
	);

	return {
		kind: 'assistant',
		id: record.id,
		content: record.content,
		createdAt: record.created_at,
		tools,
		proposals,
		citations,
		tokens: readTokenCounts(record.metadata),
		planId: readPlanId(record.metadata),
		plan: null,
		planBusy: false,
		planError: null,
		stopped: record.metadata?.['stopped'] === true
	};
}
