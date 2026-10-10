import { listReportAppraisals } from '#lib/api/generated/appraisal/appraisal.js';
import { listFullTextScreeningQueue } from '#lib/api/generated/documents/documents.js';
import {
	listExtractionFields,
	listStudyExtractionValues
} from '#lib/api/generated/extraction/extraction.js';
import { listProjectStudies } from '#lib/api/generated/studies/studies.js';
import type { ExtractionFieldDto, ExtractionValueDto } from '#lib/api/generated/models/index.js';
import type { SynthesisProgress } from '../projects/review-progress';

/** Upper bound on per-item requests; larger projects report progress on the first items only. */
const ITEM_LIMIT = 50;

/** A study is extracted once every required field has an approved value. */
export function isStudyExtracted(
	requiredFields: Pick<ExtractionFieldDto, 'id'>[],
	values: Pick<ExtractionValueDto, 'field_definition_id'>[]
): boolean {
	const filled = new Set(values.map((value) => value.field_definition_id));
	return requiredFields.every((field) => filled.has(field.id));
}

/**
 * Counts how many included reports carry a completed appraisal and how many studies have every
 * required extraction field filled. The studies list does not report its members, so appraisal is
 * counted per included report (as the appraisal screen works) and extraction per study.
 */
export async function loadSynthesisProgress(projectId: string): Promise<SynthesisProgress> {
	const [queue, studies, fields] = await Promise.all([
		listFullTextScreeningQueue(projectId, { limit: 100 }),
		listProjectStudies(projectId, { limit: ITEM_LIMIT }),
		listExtractionFields(projectId)
	]);
	const includedReports = queue.data.items
		.filter((item) => item.full_text_status === 'include')
		.slice(0, ITEM_LIMIT);
	const requiredFields = fields.data.filter((field) => field.required);

	const [appraisals, extractions] = await Promise.all([
		Promise.all(
			includedReports.map(async (report) => {
				const response = await listReportAppraisals(projectId, report.report_id);
				return response.data.length > 0;
			})
		),
		requiredFields.length === 0
			? Promise.resolve([] as boolean[])
			: Promise.all(
					studies.data.items.map(async (study) => {
						const response = await listStudyExtractionValues(projectId, study.id);
						return isStudyExtracted(requiredFields, response.data);
					})
				)
	]);

	return {
		appraised: appraisals.filter(Boolean).length,
		extracted: requiredFields.length === 0 ? undefined : extractions.filter(Boolean).length
	};
}
