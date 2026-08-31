import { describe, expect, it } from 'vitest';
import type { GraphNodeDto } from '$lib/api/generated/models';
import {
	appraisalStatus,
	overlaySummary,
	provenanceStatus,
	screeningStatus,
	studyStatus
} from './graph-overlays';

const node = (overlays: Partial<GraphNodeDto>): GraphNodeDto => ({
	report_id: '00000000-0000-0000-0000-000000000001',
	...overlays
});

describe('graph overlay mapping', () => {
	it('distinguishes screening include, exclude, and pending states', () => {
		expect(
			screeningStatus(
				node({
					screening: {
						title_abstract_status: 'include',
						full_text_status: 'include',
						final_status: 'include'
					}
				})
			)
		).toBe('include');
		expect(
			screeningStatus(
				node({
					screening: {
						title_abstract_status: 'exclude',
						full_text_status: 'not_required',
						final_status: 'exclude'
					}
				})
			)
		).toBe('exclude');
		expect(screeningStatus(node({}))).toBe('not-loaded');
	});

	it('distinguishes grouping, appraisal, and provenance', () => {
		expect(
			studyStatus(
				node({
					study: { study_id: '00000000-0000-0000-0000-000000000002', title: 'Study' }
				})
			)
		).toBe('grouped');
		expect(studyStatus(node({ study: { study_id: null, title: null } }))).toBe('ungrouped');
		expect(
			appraisalStatus(
				node({
					appraisal: {
						assessment_count: 1,
						completed_count: 1,
						latest_completed_at: null
					}
				})
			)
		).toBe('appraised');
		expect(
			provenanceStatus(node({ provenance: { sources: ['upload'], source_record_count: 1 } }))
		).toBe('acquired');
	});

	it('keeps loaded overlay values inspectable in the selected-node summary', () => {
		const summary = overlaySummary(node({}), [
			'screening',
			'study',
			'appraisal',
			'provenance',
			'metrics'
		]);
		expect(summary).toEqual([
			'Screening: not-loaded',
			'Study: not-loaded',
			'Appraisal: not-loaded',
			'Provenance: not-loaded',
			'Metrics: not loaded'
		]);
	});
});
