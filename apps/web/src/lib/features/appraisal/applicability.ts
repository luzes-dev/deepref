import type { AppraisalDefinitionDto } from '$lib/api/generated/models';

/** Study design values (as the API sends them) with reader-facing labels. */
const DESIGN_LABELS: Record<string, string> = {
	rct: 'randomized controlled trials',
	non_randomized_intervention: 'non-randomized intervention studies',
	cohort: 'cohort studies',
	case_control: 'case-control studies',
	cross_sectional: 'cross-sectional studies',
	diagnostic_accuracy: 'diagnostic accuracy studies',
	prediction_model: 'prediction model studies',
	qualitative: 'qualitative studies',
	systematic_review: 'systematic reviews',
	case_series: 'case series'
};

export function designLabel(design: string): string {
	return DESIGN_LABELS[design] ?? design;
}

export type ApplicabilityStatus =
	| { kind: 'matches'; studyLabel: string }
	| { kind: 'mismatch'; studyLabel: string; frameworkLabels: string[] }
	| { kind: 'unclassified' };

/**
 * Compares a framework's applicable designs with the report's study design.
 * An unclassified study cannot be checked, so the caller should say so rather than warn.
 */
export function applicabilityStatus(
	definition: AppraisalDefinitionDto,
	study: { design?: string | null; design_label?: string | null } | undefined
): ApplicabilityStatus {
	const design = study?.design;
	if (!design) return { kind: 'unclassified' };
	const studyLabel = study?.design_label ?? designLabel(design);
	if (definition.applicability.designs.includes(design)) {
		return { kind: 'matches', studyLabel };
	}
	return {
		kind: 'mismatch',
		studyLabel,
		frameworkLabels: definition.applicability.designs.map(designLabel)
	};
}
