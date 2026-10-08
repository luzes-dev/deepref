import type { PrismaDto, ProtocolStatusDto } from '$lib/api/generated/models';

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
	const published = protocol?.status === 'published';
	// `screened_records` counts every record that entered screening; `title_abstract_pending`
	// is the undecided share of those (unscreened plus maybe).
	const records = prisma.screened_records;
	const titleAbstractOpen = prisma.title_abstract_pending;
	const maybes = screening ? Math.min(screening.maybe, titleAbstractOpen) : 0;
	const unscreened = titleAbstractOpen - maybes;
	const sought = prisma.reports_sought;
	const readyForFullText = Math.max(
		0,
		sought - prisma.full_text_assessed - prisma.reports_not_retrieved - prisma.full_text_pending
	);

	const protocolStage: ReviewStage = published
		? {
				id: 'protocol',
				label: 'Protocol',
				state: 'done',
				summary: `Version ${protocol.version} published${
					protocol.publishedAt
						? ` ${new Date(protocol.publishedAt).toLocaleDateString(undefined, { dateStyle: 'medium' })}`
						: ''
				}`
			}
		: {
				id: 'protocol',
				label: 'Protocol',
				state: 'active',
				summary: protocol ? `Version ${protocol.version} is a draft` : 'Not written yet',
				todo: { label: protocol ? 'Publish the protocol' : 'Write the protocol' }
			};

	const waitingForCheck = prisma.unresolved_records;
	const collectStage: ReviewStage =
		prisma.identified_records === 0
			? {
					id: 'imports',
					label: 'Records',
					state: 'active',
					summary: 'No records imported',
					todo: { label: 'Import articles' }
				}
			: prisma.pending_dedupe_proposals > 0 || waitingForCheck > 0
				? {
						id: 'deduplication',
						label: 'Records',
						state: 'active',
						// Records that are not yet reports are not in Articles, so they must not read as done.
						summary:
							waitingForCheck > 0
								? `${plural(waitingForCheck, 'record')} waiting for the duplicate check`
								: `${plural(prisma.identified_records, 'record')} identified · ${plural(prisma.duplicates_removed, 'duplicate')} removed`,
						todo:
							waitingForCheck > 0
								? {
										label: `Run deduplication (${plural(waitingForCheck, 'new record')})`
									}
								: {
										label: `Resolve ${plural(prisma.pending_dedupe_proposals, 'possible duplicate')}`
									}
					}
				: {
						id: 'imports',
						label: 'Records',
						state: 'done',
						summary: `${plural(prisma.identified_records, 'record')} identified · ${plural(prisma.duplicates_removed, 'duplicate')} removed`
					};

	const titleAbstractStage: ReviewStage = {
		id: 'title-abstract',
		label: 'Title & abstract',
		state: records === 0 ? 'waiting' : titleAbstractOpen > 0 ? 'active' : 'done',
		summary:
			records === 0
				? 'Waiting for records'
				: `${plural(sought, 'record')} passed · ${plural(prisma.title_abstract_excluded, 'exclusion')}`,
		progress: share(records - titleAbstractOpen, records),
		todo:
			unscreened > 0
				? { label: `Screen ${plural(unscreened, 'record')}` }
				: maybes > 0
					? {
							label: `Decide ${plural(maybes, 'maybe', 'maybes')}`,
							search: { status: 'maybe' }
						}
					: undefined
	};

	const fullTextStage: ReviewStage = {
		id: 'full-text',
		label: 'Full text',
		state:
			sought === 0
				? 'waiting'
				: readyForFullText + prisma.reports_not_retrieved + prisma.full_text_pending > 0
					? 'active'
					: 'done',
		summary:
			sought === 0
				? 'Waiting for title & abstract inclusions'
				: `${prisma.full_text_assessed.toLocaleString()} of ${plural(sought, 'report')} assessed · ${plural(prisma.full_text_excluded, 'exclusion')}`,
		progress: share(prisma.full_text_assessed, sought),
		todo:
			readyForFullText > 0
				? { label: `Assess ${plural(readyForFullText, 'report')}` }
				: prisma.reports_not_retrieved > 0
					? {
							label: `Attach ${plural(prisma.reports_not_retrieved, 'missing PDF')}`,
							search: { filter: 'missing' }
						}
					: prisma.full_text_pending > 0
						? {
								label: `Decide ${plural(prisma.full_text_pending, 'maybe', 'maybes')}`
							}
						: undefined
	};

	const studiesStage: ReviewStage = {
		id: 'studies',
		label: 'Studies',
		state:
			prisma.full_text_included === 0
				? 'waiting'
				: prisma.included_reports_not_grouped > 0
					? 'active'
					: 'done',
		summary:
			prisma.full_text_included === 0
				? 'Waiting for full-text inclusions'
				: `${plural(prisma.full_text_included, 'included report')} in ${plural(prisma.included_studies, 'study', 'studies')}`,
		progress: share(
			prisma.full_text_included - prisma.included_reports_not_grouped,
			prisma.full_text_included
		),
		todo:
			prisma.included_reports_not_grouped > 0
				? { label: `Group ${plural(prisma.included_reports_not_grouped, 'report')}` }
				: undefined
	};

	const studyCount = prisma.included_studies;
	const todoStudies = (done: number) => Math.max(0, studyCount - done);
	const synthesisStage = (
		id: 'appraisal' | 'extraction',
		label: string,
		verb: string,
		participle: string,
		done: number | undefined
	): ReviewStage => {
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
		const left = todoStudies(done);
		return {
			id,
			label,
			state: left === 0 ? 'done' : 'active',
			summary: `${Math.min(done, studyCount).toLocaleString()} of ${studyCount.toLocaleString()} ${participle}`,
			progress: share(done, studyCount),
			todo: left > 0 ? { label: `${verb} ${plural(left, 'study', 'studies')}` } : undefined
		};
	};

	return [
		protocolStage,
		collectStage,
		titleAbstractStage,
		fullTextStage,
		studiesStage,
		synthesisStage('appraisal', 'Appraisal', 'Appraise', 'appraised', synthesis?.appraised),
		synthesisStage('extraction', 'Extraction', 'Extract', 'extracted', synthesis?.extracted)
	];
}

/** The first stage with concrete outstanding work, which the overview offers as its single action. */
export function nextStage(stages: ReviewStage[]): ReviewStage | undefined {
	return stages.find((stage) => stage.todo) ?? stages.find((stage) => stage.state === 'active');
}
