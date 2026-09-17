import { customFetch } from '$lib/api/custom-fetch';

export interface AssistantConversation {
	id: string;
	project_id: string;
	title: string;
	created_at: string;
	updated_at: string;
}

export interface AssistantToolCallRecord {
	id: string;
	tool: string;
	args: Record<string, unknown> | null;
}

export interface AssistantToolResultRecord {
	tool_call_id: string;
	tool: string;
	output: unknown;
	proposal_review_run_id?: string | null;
}

export interface AssistantMessageRecord {
	id: string;
	conversation_id: string;
	role: 'user' | 'assistant' | 'system' | 'tool';
	content: string;
	tool_calls: AssistantToolCallRecord[] | null;
	tool_results: AssistantToolResultRecord[] | null;
	metadata: Record<string, unknown> | null;
	created_at: string;
}

export type ReviewDestination =
	'screening' | 'deduplication' | 'studies' | 'extraction' | 'appraisal';

const ACTOR_HEADERS = {
	'x-actor-kind': 'user',
	'x-actor-id': 'local-user'
} as const;

const REVIEW_DESTINATIONS: Record<string, ReviewDestination> = {
	propose_screening_decision: 'screening',
	propose_duplicate_merge: 'deduplication',
	propose_study_grouping: 'studies',
	propose_classification: 'studies',
	propose_extraction: 'extraction',
	propose_appraisal_answer: 'appraisal'
};

export function reviewDestinationForTool(tool: string): ReviewDestination | null {
	return REVIEW_DESTINATIONS[tool] ?? null;
}

export function reviewQueuePathForTool(tool: string, projectId: string): string | null {
	const destination = reviewDestinationForTool(tool);
	return destination ? `/projects/${projectId}/${destination}` : null;
}

interface CustomFetchEnvelope<T> {
	data: T;
	status: number;
}

export async function listAssistantConversations(
	projectId: string
): Promise<AssistantConversation[]> {
	const response = await customFetch<CustomFetchEnvelope<AssistantConversation[]>>(
		`/api/projects/${projectId}/assistant/conversations`,
		{ method: 'GET', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export async function createAssistantConversation(
	projectId: string,
	title: string
): Promise<AssistantConversation> {
	const response = await customFetch<CustomFetchEnvelope<AssistantConversation>>(
		`/api/projects/${projectId}/assistant/conversations`,
		{
			method: 'POST',
			headers: { ...ACTOR_HEADERS, 'Content-Type': 'application/json' },
			body: JSON.stringify({ title })
		}
	);
	return response.data;
}

export async function listAssistantMessages(
	projectId: string,
	conversationId: string
): Promise<AssistantMessageRecord[]> {
	const response = await customFetch<CustomFetchEnvelope<AssistantMessageRecord[]>>(
		`/api/projects/${projectId}/assistant/conversations/${conversationId}/messages`,
		{ method: 'GET', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export async function deleteAssistantConversation(
	projectId: string,
	conversationId: string
): Promise<void> {
	await customFetch<CustomFetchEnvelope<undefined>>(
		`/api/projects/${projectId}/assistant/conversations/${conversationId}`,
		{ method: 'DELETE', headers: { ...ACTOR_HEADERS } }
	);
}

export function deriveConversationTitle(message: string): string {
	const trimmed = message.trim().replace(/\s+/g, ' ');
	if (!trimmed) return 'New chat';
	if (trimmed.length <= 60) return trimmed;
	const boundary = trimmed.lastIndexOf(' ', 60);
	const cut = boundary > 20 ? trimmed.slice(0, boundary) : trimmed.slice(0, 60);
	return `${cut.trimEnd()}…`;
}
