import type { GraphNodeDto } from '$lib/api/generated/models';
import type { GraphOverlayField } from './context.svelte.js';

export type OverlayStatus =
	| 'include'
	| 'exclude'
	| 'pending'
	| 'grouped'
	| 'ungrouped'
	| 'appraised'
	| 'not-appraised'
	| 'acquired'
	| 'no-source'
	| 'not-loaded';

export function screeningStatus(
	node: GraphNodeDto
): Extract<OverlayStatus, 'include' | 'exclude' | 'pending' | 'not-loaded'> {
	if (!node.screening) return 'not-loaded';
	const status = node.screening?.final_status;
	if (status === 'include') return 'include';
	if (status === 'exclude') return 'exclude';
	return 'pending';
}

export function studyStatus(
	node: GraphNodeDto
): Extract<OverlayStatus, 'grouped' | 'ungrouped' | 'not-loaded'> {
	if (!node.study) return 'not-loaded';
	return node.study?.study_id ? 'grouped' : 'ungrouped';
}

export function appraisalStatus(
	node: GraphNodeDto
): Extract<OverlayStatus, 'appraised' | 'not-appraised' | 'not-loaded'> {
	if (!node.appraisal) return 'not-loaded';
	return node.appraisal && node.appraisal.completed_count > 0 ? 'appraised' : 'not-appraised';
}

export function provenanceStatus(
	node: GraphNodeDto
): Extract<OverlayStatus, 'acquired' | 'no-source' | 'not-loaded'> {
	if (!node.provenance) return 'not-loaded';
	return node.provenance && node.provenance.source_record_count > 0 ? 'acquired' : 'no-source';
}

export function overlaySummary(node: GraphNodeDto, fields: readonly GraphOverlayField[]): string[] {
	const study = studyStatus(node);
	const appraisal = appraisalStatus(node);
	const provenance = provenanceStatus(node);
	const studyDetail = node.study?.title ? ` — ${node.study.title}` : '';
	const appraisalDetail = node.appraisal
		? ` (${node.appraisal.completed_count}/${node.appraisal.assessment_count} complete)`
		: '';
	const provenanceDetail = node.provenance
		? ` (${node.provenance.source_record_count} records${node.provenance.sources.length ? `: ${node.provenance.sources.join(', ')}` : ''})`
		: '';
	const summary: string[] = [];
	if (fields.includes('screening')) summary.push(`Screening: ${screeningStatus(node)}`);
	if (fields.includes('study')) summary.push(`Study: ${study}${studyDetail}`);
	if (fields.includes('appraisal')) summary.push(`Appraisal: ${appraisal}${appraisalDetail}`);
	if (fields.includes('provenance')) summary.push(`Provenance: ${provenance}${provenanceDetail}`);
	if (fields.includes('metrics'))
		summary.push(`Metrics: ${node.metrics?.rank_score?.toFixed(3) ?? 'not loaded'}`);
	return summary;
}
