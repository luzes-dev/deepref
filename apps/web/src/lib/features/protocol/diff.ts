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

function textChanges(previous: ProtocolDto, next: ProtocolDto): ProtocolChange[] {
	const text = [
		['Name', previous.name, next.name],
		['Question', previous.question, next.question],
		['Objective', previous.objective, next.objective]
	] as const;
	return text
		.filter(([, before, after]) => before.trim() !== after.trim())
		.map(([subject]) => ({ kind: 'changed' as const, subject }));
}

function frameworkChanges(previous: ProtocolDto, next: ProtocolDto): ProtocolChange[] {
	const changes: ProtocolChange[] = [];
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
		const kind = !before ? 'added' : !after ? 'removed' : 'changed';
		changes.push({ kind, subject: humanizeKey(key) });
	}
	return changes;
}

/** What differs between two versions of one criterion, or `undefined` when nothing does. */
function criterionDetail(before: Criterion, after: Criterion): string | undefined {
	const parts: string[] = [];
	if (before.stage !== after.stage) parts.push(`stage: ${criterionStageLabel(after.stage)}`);
	if (before.dimension !== after.dimension)
		parts.push(`dimension: ${criterionDimensionLabel(after.dimension)}`);
	if (before.description.trim() !== after.description.trim()) parts.push('description');
	return parts.length > 0 ? parts.join(', ') : undefined;
}

function criteriaChanges(previous: ProtocolDto, next: ProtocolDto): ProtocolChange[] {
	const changes: ProtocolChange[] = [];
	const beforeCriteria = new Map(previous.criteria.map((c) => [criterionKey(c), c]));
	const afterCriteria = new Map(next.criteria.map((c) => [criterionKey(c), c]));
	for (const [key, criterion] of afterCriteria) {
		const before = beforeCriteria.get(key);
		const subject = criterionSubject(criterion);
		if (!before) {
			changes.push({ kind: 'added', subject });
			continue;
		}
		const detail = criterionDetail(before, criterion);
		if (detail) changes.push({ kind: 'changed', subject, detail });
	}
	for (const [key, criterion] of beforeCriteria) {
		if (!afterCriteria.has(key))
			changes.push({ kind: 'removed', subject: criterionSubject(criterion) });
	}
	return changes;
}

/** Field-level differences between two versions of the same protocol. */
export function diffProtocols(previous: ProtocolDto, next: ProtocolDto): ProtocolChange[] {
	return [
		...textChanges(previous, next),
		...frameworkChanges(previous, next),
		...criteriaChanges(previous, next)
	];
}
