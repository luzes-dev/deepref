import type { ProtocolDto } from './api';
import {
	FRAMEWORK_FIELDS,
	criterionDimensionLabel,
	criterionKindLabel,
	criterionStageLabel,
	frameworkLabel,
	humanizeKey
} from './codecs';

export type ProtocolChange = {
	kind: 'added' | 'removed' | 'changed';
	/** What changed, e.g. "Question" or "Criterion: Adults only". */
	subject: string;
	detail?: string;
};

type Criterion = ProtocolDto['criteria'][number];

function criterionKey(criterion: Criterion): string {
	return `${criterion.kind}:${criterion.label.trim().toLowerCase()}`;
}

function criterionSubject(criterion: Criterion): string {
	return `${criterionKindLabel(criterion.kind)} criterion: ${criterion.label}`;
}

function frameworkKeys(protocol: ProtocolDto): string[] {
	const order: readonly string[] =
		protocol.framework_kind in FRAMEWORK_FIELDS
			? FRAMEWORK_FIELDS[protocol.framework_kind as keyof typeof FRAMEWORK_FIELDS]
			: [];
	return [...new Set([...order, ...Object.keys(protocol.framework_fields ?? {})])];
}

/** Field-level differences between two versions of the same protocol. */
export function diffProtocols(previous: ProtocolDto, next: ProtocolDto): ProtocolChange[] {
	const changes: ProtocolChange[] = [];
	const text = [
		['Name', previous.name, next.name],
		['Question', previous.question, next.question],
		['Objective', previous.objective, next.objective]
	] as const;
	for (const [subject, before, after] of text) {
		if (before.trim() !== after.trim()) changes.push({ kind: 'changed', subject });
	}
	if (previous.framework_kind !== next.framework_kind) {
		changes.push({
			kind: 'changed',
			subject: 'Framework',
			detail: `${frameworkLabel(previous.framework_kind)} to ${frameworkLabel(next.framework_kind)}`
		});
	}
	const beforeFields = previous.framework_fields ?? {};
	const afterFields = next.framework_fields ?? {};
	for (const key of new Set([...frameworkKeys(previous), ...frameworkKeys(next)])) {
		const before = beforeFields[key]?.trim();
		const after = afterFields[key]?.trim();
		if (before === after) continue;
		const subject = humanizeKey(key);
		if (!before) changes.push({ kind: 'added', subject });
		else if (!after) changes.push({ kind: 'removed', subject });
		else changes.push({ kind: 'changed', subject });
	}
	const beforeCriteria = new Map(previous.criteria.map((c) => [criterionKey(c), c]));
	const afterCriteria = new Map(next.criteria.map((c) => [criterionKey(c), c]));
	for (const [key, criterion] of afterCriteria) {
		const before = beforeCriteria.get(key);
		if (!before) {
			changes.push({ kind: 'added', subject: criterionSubject(criterion) });
			continue;
		}
		const parts: string[] = [];
		if (before.stage !== criterion.stage)
			parts.push(`stage: ${criterionStageLabel(criterion.stage)}`);
		if (before.dimension !== criterion.dimension)
			parts.push(`dimension: ${criterionDimensionLabel(criterion.dimension)}`);
		if (before.description.trim() !== criterion.description.trim()) parts.push('description');
		if (parts.length > 0)
			changes.push({
				kind: 'changed',
				subject: criterionSubject(criterion),
				detail: parts.join(', ')
			});
	}
	for (const [key, criterion] of beforeCriteria) {
		if (!afterCriteria.has(key))
			changes.push({ kind: 'removed', subject: criterionSubject(criterion) });
	}
	return changes;
}
