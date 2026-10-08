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

export interface AssistantPlanManualStep {
	reason: string;
	link_target: string;
}

export interface AssistantPlanAction {
	id: string;
	tool: string;
	summary: string;
	rationale: string;
	affected_count: number;
	executable: boolean;
	manual: AssistantPlanManualStep | null;
}

export interface AssistantPlanResult {
	action_id: string;
	status: 'executed' | 'queued' | 'completed' | 'failed' | 'skipped' | 'manual';
	message: string;
	review_run_id: string | null;
	applied: number | null;
	unchanged: number | null;
	failed: number | null;
}

export type AssistantPlanStatus = 'pending' | 'confirmed' | 'rejected' | 'executed' | 'failed';

export interface AssistantPlan {
	id: string;
	project_id: string;
	conversation_id: string;
	status: AssistantPlanStatus;
	summary: string;
	actions: AssistantPlanAction[];
	results: AssistantPlanResult[] | null;
	error: string | null;
	model: string;
	prompt_version: string;
	created_at: string;
	resolved_by: string | null;
	resolved_at: string | null;
}

export function isAssistantPlan(value: unknown): value is AssistantPlan {
	if (typeof value !== 'object' || value === null) return false;
	const candidate = value as Record<string, unknown>;
	return (
		typeof candidate['id'] === 'string' &&
		typeof candidate['status'] === 'string' &&
		Array.isArray(candidate['actions'])
	);
}

export function manualStepPath(projectId: string, linkTarget: string): string {
	return linkTarget === 'protocol'
		? `/projects/${projectId}/protocol`
		: `/projects/${projectId}/screening/full-text`;
}

export function budgetReachedMessage(error: unknown): string | null {
	if (typeof error !== 'object' || error === null) return null;
	const record = error as { code?: unknown; status?: unknown };
	return record.code === 'ai_budget_exceeded' ? 'AI budget for this month reached' : null;
}

export async function getAssistantPlan(projectId: string, planId: string): Promise<AssistantPlan> {
	const response = await customFetch<CustomFetchEnvelope<AssistantPlan>>(
		`/api/projects/${projectId}/assistant/plans/${planId}`,
		{ method: 'GET', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export async function confirmAssistantPlan(
	projectId: string,
	planId: string
): Promise<AssistantPlan> {
	const response = await customFetch<CustomFetchEnvelope<AssistantPlan>>(
		`/api/projects/${projectId}/assistant/plans/${planId}/confirm`,
		{ method: 'POST', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export async function rejectAssistantPlan(
	projectId: string,
	planId: string
): Promise<AssistantPlan> {
	const response = await customFetch<CustomFetchEnvelope<AssistantPlan>>(
		`/api/projects/${projectId}/assistant/plans/${planId}/reject`,
		{ method: 'POST', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export interface AiBudget {
	monthly_budget_usd: number;
	spent_usd: number;
	remaining_usd: number;
	exhausted: boolean;
}

export async function fetchAiBudget(projectId: string): Promise<AiBudget> {
	const response = await customFetch<CustomFetchEnvelope<AiBudget>>(
		`/api/projects/${projectId}/ai/budget`,
		{ method: 'GET', headers: { ...ACTOR_HEADERS } }
	);
	return response.data;
}

export function formatUsd(value: number): string {
	return new Intl.NumberFormat(undefined, {
		style: 'currency',
		currency: 'USD',
		minimumFractionDigits: 2
	}).format(value);
}
