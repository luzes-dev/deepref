import type { PrismaDto } from '$lib/api/generated/models';

type FlowCounts = Pick<
	PrismaDto,
	| 'identified_records'
	| 'manually_created_reports'
	| 'duplicates_removed'
	| 'unresolved_records'
	| 'screened_records'
>;

export type FlowTotals = {
	/** Identified records plus reports added manually: the top box of the diagram. */
	identified: number;
	duplicatesRemoved: number;
	/** Records still waiting for the duplicate check (the diagram's awaiting box). */
	awaitingDuplicateCheck: number;
	screened: number;
	/** identified = duplicates removed + awaiting duplicate check + screened. */
	reconciles: boolean;
};

/**
 * The numbers the PRISMA diagram shows, derived the same way as the server's diagram so
 * the boxes add up on the page. Manually added reports are not identified records, so they
 * join the identified total; that keeps identified - removed - awaiting = screened.
 */
export function flowTotals(counts: FlowCounts): FlowTotals {
	const identified = counts.identified_records + counts.manually_created_reports;
	const duplicatesRemoved = counts.duplicates_removed;
	const awaitingDuplicateCheck = counts.unresolved_records;
	const screened = counts.screened_records;
	return {
		identified,
		duplicatesRemoved,
		awaitingDuplicateCheck,
		screened,
		reconciles: identified === duplicatesRemoved + awaitingDuplicateCheck + screened
	};
}

/** One sentence under the diagram that states the balance with the same numbers as the boxes. */
export function reconciliationSentence(totals: FlowTotals): string {
	const removed = [`${totals.duplicatesRemoved} duplicates removed`];
	if (totals.awaitingDuplicateCheck > 0)
		removed.push(`${totals.awaitingDuplicateCheck} awaiting duplicate check`);
	const sentence = `${totals.identified} identified = ${removed.join(' + ')} + ${totals.screened} screened`;
	return totals.reconciles ? sentence : `${sentence}. The counts do not add up.`;
}
