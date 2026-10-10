import type { DependencyStatus } from '#lib/api/generated/models/index.js';

export type DependencyHealthLevel = 'ok' | 'degraded' | 'unavailable';

export interface DependencyHealthIssue {
	name: string;
	label: string;
	state: 'degraded' | 'unavailable';
	message: string;
}

export interface DependencyHealthSummary {
	level: DependencyHealthLevel;
	issues: DependencyHealthIssue[];
}

const LABELS: Record<string, string> = {
	postgresql: 'Database',
	worker: 'Background jobs'
};

export function dependencyLabel(name: string): string {
	return LABELS[name] ?? name.charAt(0).toUpperCase() + name.slice(1);
}

export interface DependencyHealthOptions {
	/** Whether the probe covered this project's jobs or the whole workspace. */
	scope?: 'project' | 'workspace';
}

type IssueDetail = { state: string; backlog?: number | null; recentFailed?: number | null };

function workerMessage(detail: IssueDetail, scope: 'project' | 'workspace'): string {
	if (detail.state === 'unavailable') return 'Background jobs cannot be checked right now.';
	const queued = detail.backlog ? ` ${detail.backlog} jobs are waiting.` : '';
	const failed = detail.recentFailed ?? 0;
	if (failed <= 0) {
		return `Imports and automations are running behind.${queued} Projects and articles stay available.`;
	}
	const where = scope === 'project' ? 'in this project' : 'in the workspace';
	const noun = failed === 1 ? 'job' : 'jobs';
	return `${failed} background ${noun} failed in the last 30 minutes ${where}.${queued} Imports and automations may be incomplete.`;
}

function issueMessage(name: string, detail: IssueDetail, scope: 'project' | 'workspace'): string {
	const unavailable = detail.state === 'unavailable';
	if (name === 'worker') return workerMessage(detail, scope);
	if (name === 'postgresql') {
		return unavailable
			? 'The database is not responding. Changes may not be saved.'
			: 'The database is responding slowly.';
	}
	return unavailable ? 'Not responding.' : 'Running with reduced capacity.';
}

/**
 * Reduce the dependency probe to what a researcher needs to know. Only
 * non-available dependencies (or an unreachable status endpoint) surface;
 * recovery simply returns an empty summary.
 */
export function summarizeDependencyHealth(
	status: DependencyStatus | undefined,
	error: Error | null | undefined,
	options: DependencyHealthOptions = {}
): DependencyHealthSummary {
	const scope = options.scope ?? 'workspace';
	if (error) {
		return {
			level: 'unavailable',
			issues: [
				{
					name: 'status',
					label: 'Server',
					state: 'unavailable',
					message: 'Cannot reach the server to check system status.'
				}
			]
		};
	}
	if (!status) return { level: 'ok', issues: [] };

	const issues: DependencyHealthIssue[] = [];
	for (const [name, detail] of Object.entries(status)) {
		if (detail.state === 'available') continue;
		const state = detail.state === 'unavailable' ? 'unavailable' : 'degraded';
		issues.push({
			name,
			label: dependencyLabel(name),
			state,
			message: issueMessage(
				name,
				{ state, backlog: detail.backlog, recentFailed: detail.recent_failed },
				scope
			)
		});
	}
	const level = issues.some((issue) => issue.state === 'unavailable')
		? 'unavailable'
		: issues.length > 0
			? 'degraded'
			: 'ok';
	return { level, issues };
}
