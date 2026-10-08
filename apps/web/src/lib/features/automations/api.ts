import { ApiError } from '$lib/api/custom-fetch';
import type { WorkflowDto, WorkflowRunDto } from '$lib/api/generated/models';
import type { WfGraph } from './model';

export interface Issue {
	node_id?: string | null;
	message: string;
}

/** Problems reported by the server when a publish or test is refused. */
export function issuesFromError(error: unknown): Issue[] | null {
	if (!(error instanceof ApiError) || error.code !== 'WORKFLOW_INVALID') return null;
	const details = (error.info as { details?: { issues?: Issue[] } } | undefined)?.details;
	return Array.isArray(details?.issues) ? details.issues : [];
}

export function isConflict(error: unknown): boolean {
	return error instanceof ApiError && error.code === 'WORKFLOW_CONFLICT';
}

export function graphOf(
	workflow: Pick<WorkflowDto, 'graph'> | Pick<WorkflowRunDto, 'graph'>
): WfGraph {
	return workflow.graph;
}

export const RUN_STATUS_LABEL: Record<string, string> = {
	queued: 'Waiting to start',
	running: 'Running',
	completed: 'Finished',
	failed: 'Stopped by a problem',
	cancelled: 'Cancelled'
};

export function isActiveRun(status: string): boolean {
	return status === 'queued' || status === 'running';
}

export function formatWhen(value: string | null | undefined): string {
	if (!value) return '—';
	const date = new Date(value);
	if (Number.isNaN(date.getTime())) return '—';
	return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
		date
	);
}

export function absoluteHookUrl(path: string): string {
	if (typeof window === 'undefined') return path;
	return `${window.location.origin}/api${path}`;
}
