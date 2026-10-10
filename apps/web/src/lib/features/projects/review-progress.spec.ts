import { describe, expect, it } from 'vitest';
import type { PrismaDto } from '#lib/api/generated/models/index.js';
import { nextStage, reviewStages } from './review-progress';

function prisma(overrides: Partial<PrismaDto> = {}): PrismaDto {
	return {
		project_id: 'project-1',
		reconciliation_warnings: [],
		identified_records: 12,
		linked_records: 12,
		duplicates_removed: 0,
		unresolved_records: 0,
		pending_dedupe_proposals: 0,
		source_canonical_reports: 12,
		manually_created_reports: 0,
		screened_records: 12,
		screening_high_watermark: 0,
		title_abstract_excluded: 1,
		title_abstract_pending: 0,
		reports_sought: 10,
		reports_not_retrieved: 0,
		full_text_assessed: 10,
		full_text_pending: 0,
		full_text_excluded: 2,
		full_text_exclusions: [],
		full_text_included: 8,
		included_reports_not_grouped: 0,
		included_studies: 3,
		...overrides
	};
}

const published = { status: 'published', version: 1, publishedAt: null } as const;

describe('review progress', () => {
	it('asks for a protocol before anything else', () => {
		const stages = reviewStages(prisma(), null);
		expect(nextStage(stages)?.todo?.label).toBe('Write the protocol');
	});

	it('routes possible duplicates to deduplication', () => {
		const next = nextStage(reviewStages(prisma({ pending_dedupe_proposals: 3 }), published));
		expect(next?.id).toBe('deduplication');
		expect(next?.todo?.label).toBe('Resolve 3 possible duplicates');
	});

	it('keeps unresolved imports active until the duplicate check has run', () => {
		const stages = reviewStages(
			prisma({
				identified_records: 3,
				linked_records: 0,
				unresolved_records: 3,
				source_canonical_reports: 0,
				screened_records: 0,
				title_abstract_pending: 0,
				reports_sought: 0,
				full_text_assessed: 0,
				full_text_included: 0,
				included_studies: 0
			}),
			published
		);
		const records = stages.find((stage) => stage.id === 'deduplication');
		expect(records?.state).toBe('active');
		expect(records?.summary).toBe('3 records waiting for the duplicate check');
		expect(nextStage(stages)?.todo?.label).toBe('Run deduplication (3 new records)');
	});

	it('counts unscreened records before maybes at title and abstract', () => {
		const fresh = prisma({ title_abstract_pending: 12, reports_sought: 0 });
		expect(nextStage(reviewStages(fresh, published))?.todo).toEqual({
			label: 'Screen 12 records'
		});
		expect(
			nextStage(reviewStages(fresh, published, { unscreened: 12, maybe: 0 }))?.todo
		).toEqual({ label: 'Screen 12 records' });

		const maybes = nextStage(
			reviewStages(prisma({ title_abstract_pending: 2 }), published, {
				unscreened: 0,
				maybe: 2
			})
		);
		expect(maybes?.todo).toEqual({ label: 'Decide 2 maybes', search: { status: 'maybe' } });
	});

	it('sends missing PDFs to the full-text missing filter once nothing is ready to assess', () => {
		const next = nextStage(
			reviewStages(
				prisma({ reports_not_retrieved: 8, full_text_assessed: 2, full_text_included: 2 }),
				published
			)
		);
		expect(next?.id).toBe('full-text');
		expect(next?.todo).toEqual({
			label: 'Attach 8 missing PDFs',
			search: { filter: 'missing' }
		});
	});

	it('asks for grouping when included reports are not in a study', () => {
		const next = nextStage(
			reviewStages(prisma({ included_reports_not_grouped: 4 }), published)
		);
		expect(next?.todo?.label).toBe('Group 4 reports');
	});

	it('marks screening stages done and leaves synthesis active when everything is grouped', () => {
		const stages = reviewStages(prisma(), published);
		expect(stages.slice(0, 5).map((stage) => stage.state)).toEqual([
			'done',
			'done',
			'done',
			'done',
			'done'
		]);
		expect(nextStage(stages)?.id).toBe('appraisal');
		expect(nextStage(stages)?.todo).toBeUndefined();
	});

	it('keeps every stage on a brand-new project with the protocol first', () => {
		const empty = prisma({
			identified_records: 0,
			linked_records: 0,
			source_canonical_reports: 0,
			screened_records: 0,
			title_abstract_excluded: 0,
			reports_sought: 0,
			full_text_assessed: 0,
			full_text_excluded: 0,
			full_text_included: 0,
			included_studies: 0
		});
		const stages = reviewStages(empty, null);
		expect(stages).toHaveLength(7);
		expect(stages.map((stage) => stage.state)).toEqual([
			'active',
			'active',
			'waiting',
			'waiting',
			'waiting',
			'waiting',
			'waiting'
		]);
		expect(nextStage(stages)?.todo?.label).toBe('Write the protocol');
		expect(stages[1]?.todo?.label).toBe('Import articles');
	});

	it('reports appraisal and extraction progress per study', () => {
		const stages = reviewStages(prisma({ included_studies: 4 }), published, undefined, {
			appraised: 1,
			extracted: 0
		});
		const [appraisal, extraction] = stages.slice(5);
		expect(appraisal).toMatchObject({
			state: 'active',
			summary: '1 of 4 appraised',
			progress: 0.25,
			todo: { label: 'Appraise 3 studies' }
		});
		expect(extraction).toMatchObject({
			summary: '0 of 4 extracted',
			progress: 0,
			todo: { label: 'Extract 4 studies' }
		});
		expect(nextStage(stages)?.id).toBe('appraisal');
	});

	it('marks appraisal done once the only included study is appraised', () => {
		const stages = reviewStages(prisma({ included_studies: 1 }), published, undefined, {
			appraised: 1,
			extracted: 0
		});
		expect(stages[5]).toMatchObject({ state: 'done', summary: '1 of 1 appraised' });
		expect(stages[5]?.todo).toBeUndefined();
		expect(nextStage(stages)?.todo?.label).toBe('Extract 1 study');
	});

	it('falls back to a generic line when extraction cannot be measured', () => {
		const stages = reviewStages(prisma({ included_studies: 2 }), published, undefined, {
			appraised: 2
		});
		expect(stages[6]).toMatchObject({ state: 'active', summary: '2 studies to work through' });
	});
});
