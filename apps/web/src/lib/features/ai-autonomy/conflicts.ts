export type ConflictStatus = 'conflict' | 'waiting' | 'concordant' | 'resolved';

export const CONFLICT_STATUS_LABEL: Record<ConflictStatus, string> = {
	conflict: 'Conflicts',
	waiting: 'Waiting for you',
	concordant: 'Agreed',
	resolved: 'Settled'
};

const DECISION_LABEL: Record<string, string> = {
	include: 'Include',
	exclude: 'Exclude',
	maybe: 'Maybe'
};

export function decisionLabel(value: string | null | undefined): string {
	return value ? (DECISION_LABEL[value] ?? value) : 'No decision yet';
}

export function agreementSummary(stage: {
	compared: number;
	agreed: number;
	kappa?: number | null;
}): string {
	if (stage.compared === 0) return 'No decisions to compare yet.';
	const percent = Math.round((stage.agreed / stage.compared) * 100);
	const kappa =
		typeof stage.kappa === 'number'
			? `, agreement beyond chance (kappa) ${stage.kappa.toFixed(2)}`
			: '';
	return `You and the AI agreed on ${stage.agreed} of ${stage.compared} decisions (${percent}%)${kappa}.`;
}

/**
 * Copy for an empty list. "Agree" is only claimed when the two reviewers have
 * actually decided at least one record in common.
 */
export function emptyConflictCopy(
	status: ConflictStatus,
	compared: number
): { title: string; description: string } {
	if (status === 'waiting') {
		return {
			title: 'No AI opinions waiting for you',
			description:
				'When the AI has screened a record you have not decided yet, it appears here with its decision hidden until you decide.'
		};
	}
	if (status === 'resolved') {
		return { title: 'Nothing settled yet', description: 'Conflicts you settle appear here.' };
	}
	if (status === 'concordant') {
		return {
			title: 'No agreements yet',
			description: 'Records where you and the AI decided the same way appear here.'
		};
	}
	if (compared === 0) {
		return {
			title: 'No decisions to compare yet',
			description:
				'The AI reviewer and you have not both decided a record yet, so there is nothing to agree or disagree on.'
		};
	}
	return {
		title: 'No conflicts',
		description: 'You and the AI reviewer agree on every record you have both decided.'
	};
}

/** Splits a multi-criterion rationale into its lines, dropping blank ones. */
export function rationaleLines(text: string | null | undefined): string[] {
	return (text ?? '')
		.split(/\r?\n/)
		.map((line) => line.trim())
		.filter(Boolean);
}
