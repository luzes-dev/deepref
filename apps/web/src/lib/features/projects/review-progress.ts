import type { PrismaDto, ProtocolStatusDto } from '#lib/api/generated/models/index.js';

export type StageState = 'done' | 'active' | 'waiting';

export type StageRoute =
	| 'protocol'
	| 'imports'
	| 'deduplication'
	| 'title-abstract'
	| 'full-text'
	| 'studies'
	| 'appraisal'
	| 'extraction';

export type ReviewStage = {
	id: StageRoute;
	label: string;
	state: StageState;
	/** One line describing where the stage stands. */
	summary: string;
	/** Completed share in the range 0..1, when the stage has a measurable queue. */
	progress?: number;
	/** The outstanding work, phrased as an instruction. Absent when nothing is left. */
	todo?: { label: string; search?: Record<string, string> };
};

/** Title/abstract queue split; the PRISMA projection only reports unscreened and maybe together. */
export type ScreeningSplit = { unscreened: number; maybe: number };

/**
 * Appraisal and extraction completion; absent while loading or unavailable. `extracted` is
 * undefined when the project defines no required extraction fields, so it cannot be measured.
 */
export type SynthesisProgress = { appraised: number; extracted?: number };

export type ProtocolSummary = {
	status: ProtocolStatusDto;
	version: number;
	publishedAt?: string | null;
} | null;

function plural(count: number, one: string, many = `${one}s`): string {
	return `${count.toLocaleString()} ${count === 1 ? one : many}`;
}

function share(done: number, total: number): number | undefined {
	return total > 0 ? Math.min(1, Math.max(0, done / total)) : undefined;
}

function protocolStage(protocol: ProtocolSummary): ReviewStage {
	if (protocol?.status === 'published') {
		const date = protocol.publishedAt
			? ` ${new Date(protocol.publishedAt).toLocaleDateString(undefined, { dateStyle: 'medium' })}`
			: '';
		return {
			id: 'protocol',
			label: 'Protocol',
			state: 'done',
			summary: `Version ${protocol.version} published${date}`
		};
	}
	return {
		id: 'protocol',
		label: 'Protocol',
		state: 'active',
		summary: protocol ? `Version ${protocol.version} is a draft` : 'Not written yet',
		todo: { label: protocol ? 'Publish the protocol' : 'Write the protocol' }
	};
}

function collectStage(prisma: PrismaDto): ReviewStage {
	if (prisma.identified_records === 0) {
		return {
			id: 'imports',
			label: 'Records',
			state: 'active',
			summary: 'No records imported',
			todo: { label: 'Import articles' }
		};
	}
	const identified = `${plural(prisma.identified_records, 'record')} identified · ${plural(prisma.duplicates_removed, 'duplicate')} removed`;
	const waitingForCheck = prisma.unresolved_records;
	if (waitingForCheck > 0) {
		// Records that are not yet reports are not in Articles, so they must not read as done.
		return {
			id: 'deduplication',
			label: 'Records',
			state: 'active',
			summary: `${plural(waitingForCheck, 'record')} waiting for the duplicate check`,
			todo: { label: `Run deduplication (${plural(waitingForCheck, 'new record')})` }
		};
	}
	if (prisma.pending_dedupe_proposals > 0) {
		return {
			id: 'deduplication',
			label: 'Records',
			state: 'active',
			summary: identified,
			todo: {
				label: `Resolve ${plural(prisma.pending_dedupe_proposals, 'possible duplicate')}`
			}
		};
	}
	return { id: 'imports', label: 'Records', state: 'done', summary: identified };
}

function titleAbstractStage(prisma: PrismaDto, screening?: ScreeningSplit): ReviewStage {
	// `screened_records` counts every record that entered screening; `title_abstract_pending`
	// is the undecided share of those (unscreened plus maybe).
	const records = prisma.screened_records;
	const open = prisma.title_abstract_pending;
	const maybes = screening ? Math.min(screening.maybe, open) : 0;
	const unscreened = open - maybes;
	const stage: ReviewStage = {
		id: 'title-abstract',
		label: 'Title & abstract',
		state: open > 0 ? 'active' : 'done',
		summary: `${plural(prisma.reports_sought, 'record')} passed · ${plural(prisma.title_abstract_excluded, 'exclusion')}`,
		progress: share(records - open, records)
	};
	if (records === 0) {
		return { ...stage, state: 'waiting', summary: 'Waiting for records' };
	}
	if (unscreened > 0) {
		return { ...stage, todo: { label: `Screen ${plural(unscreened, 'record')}` } };
	}
	if (maybes > 0) {
		return {
			...stage,
			todo: {
				label: `Decide ${plural(maybes, 'maybe', 'maybes')}`,
				search: { status: 'maybe' }
			}
		};
	}
	return stage;
}

function fullTextTodo(prisma: PrismaDto, ready: number): ReviewStage['todo'] {
	if (ready > 0) {
		return { label: `Assess ${plural(ready, 'report')}` };
	}
	if (prisma.reports_not_retrieved > 0) {
		return {
			label: `Attach ${plural(prisma.reports_not_retrieved, 'missing PDF')}`,
			search: { filter: 'missing' }
		};
	}
	if (prisma.full_text_pending > 0) {
		return { label: `Decide ${plural(prisma.full_text_pending, 'maybe', 'maybes')}` };
	}
	return undefined;
}

function fullTextStage(prisma: PrismaDto): ReviewStage {
	const sought = prisma.reports_sought;
	const progress = share(prisma.full_text_assessed, sought);
	if (sought === 0) {
		return {
			id: 'full-text',
			label: 'Full text',
			state: 'waiting',
			summary: 'Waiting for title & abstract inclusions',
			progress
		};
	}
	const ready = Math.max(
		0,
		sought - prisma.full_text_assessed - prisma.reports_not_retrieved - prisma.full_text_pending
	);
	const todo = fullTextTodo(prisma, ready);
	return {
		id: 'full-text',
		label: 'Full text',
		state: todo ? 'active' : 'done',
		summary: `${prisma.full_text_assessed.toLocaleString()} of ${plural(sought, 'report')} assessed · ${plural(prisma.full_text_excluded, 'exclusion')}`,
		progress,
		todo
	};
}

function studiesStage(prisma: PrismaDto): ReviewStage {
	const included = prisma.full_text_included;
	const ungrouped = prisma.included_reports_not_grouped;
	const progress = share(included - ungrouped, included);
	if (included === 0) {
		return {
			id: 'studies',
			label: 'Studies',
			state: 'waiting',
			summary: 'Waiting for full-text inclusions',
			progress
		};
	}
	return {
		id: 'studies',
		label: 'Studies',
		state: ungrouped > 0 ? 'active' : 'done',
		summary: `${plural(included, 'included report')} in ${plural(prisma.included_studies, 'study', 'studies')}`,
		progress,
		todo: ungrouped > 0 ? { label: `Group ${plural(ungrouped, 'report')}` } : undefined
	};
}

function synthesisStage(
	studyCount: number,
	id: 'appraisal' | 'extraction',
	label: string,
	verb: string,
	participle: string,
	done: number | undefined
): ReviewStage {
	if (studyCount === 0) {
		return { id, label, state: 'waiting', summary: 'Waiting for included studies' };
	}
	if (done === undefined) {
		return {
			id,
			label,
			state: 'active',
			summary: `${plural(studyCount, 'study', 'studies')} to work through`
		};
	}
	const left = Math.max(0, studyCount - done);
	return {
		id,
		label,
		state: left === 0 ? 'done' : 'active',
		summary: `${Math.min(done, studyCount).toLocaleString()} of ${studyCount.toLocaleString()} ${participle}`,
		progress: share(done, studyCount),
		todo: left > 0 ? { label: `${verb} ${plural(left, 'study', 'studies')}` } : undefined
	};
}

/**
 * Derives the review pipeline from the PRISMA projection, so the overview can say what is left
 * at every stage and decide the next step itself instead of asking the reviewer to work it out.
 */
export function reviewStages(
	prisma: PrismaDto,
	protocol: ProtocolSummary,
	screening?: ScreeningSplit,
	synthesis?: SynthesisProgress
): ReviewStage[] {
	const studies = prisma.included_studies;
	return [
		protocolStage(protocol),
		collectStage(prisma),
		titleAbstractStage(prisma, screening),
		fullTextStage(prisma),
		studiesStage(prisma),
		synthesisStage(
			studies,
			'appraisal',
			'Appraisal',
			'Appraise',
			'appraised',
			synthesis?.appraised
		),
		synthesisStage(
			studies,
			'extraction',
			'Extraction',
			'Extract',
			'extracted',
			synthesis?.extracted
		)
	];
}

/** The first stage with concrete outstanding work, which the overview offers as its single action. */
export function nextStage(stages: ReviewStage[]): ReviewStage | undefined {
	return stages.find((stage) => stage.todo) ?? stages.find((stage) => stage.state === 'active');
}
