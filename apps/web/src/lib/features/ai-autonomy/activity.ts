import type { AiActivityDto } from '$lib/api/generated/models';

export type ActivityGroup =
	| { kind: 'single'; key: string; entry: AiActivityDto }
	| { kind: 'batch'; key: string; batchId: string; entries: AiActivityDto[] };

/**
 * Entries that belong to the same batch (one workflow run, one confirmed
 * assistant plan, one duplicate check) are shown together so they can be
 * undone with a single action. A batch of one is just a single entry.
 */
export function groupActivity(entries: readonly AiActivityDto[]): ActivityGroup[] {
	const groups: ActivityGroup[] = [];
	const byBatch = new Map<string, Extract<ActivityGroup, { kind: 'batch' }>>();
	for (const entry of entries) {
		if (!entry.batch_id) {
			groups.push({ kind: 'single', key: entry.id, entry });
			continue;
		}
		const existing = byBatch.get(entry.batch_id);
		if (existing) {
			existing.entries.push(entry);
			continue;
		}
		const group: Extract<ActivityGroup, { kind: 'batch' }> = {
			kind: 'batch',
			key: `batch-${entry.batch_id}`,
			batchId: entry.batch_id,
			entries: [entry]
		};
		byBatch.set(entry.batch_id, group);
		groups.push(group);
	}
	return groups.map((group) =>
		group.kind === 'batch' && group.entries.length === 1
			? { kind: 'single', key: group.entries[0].id, entry: group.entries[0] }
			: group
	);
}

export function undoableCount(entries: readonly AiActivityDto[]): number {
	return entries.filter((entry) => entry.undoable && !entry.undone_at).length;
}

export const ACTOR_LABEL: Record<string, string> = {
	ai: 'AI',
	automation: 'Automation',
	assistant: 'Assistant'
};

export const activityKeys = {
	all: (projectId: string) => ['ai-activity', projectId] as const,
	screeningQueue: (projectId: string) => ['screening-queue', projectId] as const
};
