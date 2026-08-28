import { AutomationDefinitionStatusInput, AutomationTriggerInput } from '$lib/api/generated/models';
import type {
	AutomationDefinitionDto,
	AutomationDefinitionStatusInput as AutomationStatus,
	AutomationRunDto,
	AutomationTriggerInput as AutomationTrigger
} from '$lib/api/generated/models';

export const AUTOMATION_RECIPE_ID = 'project_maintenance' as const;
export const AUTOMATION_RECIPE_VERSION = 1 as const;
export const AUTOMATION_RECIPE_ROUTE = 'project_maintenance.v1' as const;

export const AUTOMATION_TRIGGERS = [
	AutomationTriggerInput.report_added,
	AutomationTriggerInput.acquisition_completed,
	AutomationTriggerInput.full_text_attached,
	AutomationTriggerInput.report_included,
	AutomationTriggerInput.study_created,
	AutomationTriggerInput.appraisal_completed,
	AutomationTriggerInput.manual
] satisfies readonly AutomationTrigger[];

export const AUTOMATION_STATUSES = [
	AutomationDefinitionStatusInput.active,
	AutomationDefinitionStatusInput.paused
] satisfies readonly AutomationStatus[];

export type AutomationDraft = {
	name: string;
	trigger: AutomationTrigger;
	status: AutomationStatus;
};

export const DEFAULT_AUTOMATION_DRAFT = {
	name: 'Project maintenance',
	trigger: AutomationTriggerInput.manual,
	status: AutomationDefinitionStatusInput.active
} satisfies AutomationDraft;

const TRIGGER_LABELS = {
	report_added: 'Report added',
	acquisition_completed: 'Acquisition completed',
	full_text_attached: 'Full text attached',
	report_included: 'Report included',
	study_created: 'Study created',
	appraisal_completed: 'Appraisal completed',
	manual: 'Manual'
} satisfies Record<AutomationTrigger, string>;

export function isAutomationTrigger(value: unknown): value is AutomationTrigger {
	return typeof value === 'string' && AUTOMATION_TRIGGERS.some((trigger) => trigger === value);
}

export function isAutomationStatus(value: unknown): value is AutomationStatus {
	return typeof value === 'string' && AUTOMATION_STATUSES.some((status) => status === value);
}

export function labelForTrigger(value: string): string {
	return isAutomationTrigger(value) ? TRIGGER_LABELS[value] : humanize(value);
}

export function labelForStatus(value: string): string {
	return isAutomationStatus(value) ? (value === 'active' ? 'Active' : 'Paused') : humanize(value);
}

export function draftFromDefinition(
	definition: AutomationDefinitionDto
): AutomationDraft | undefined {
	if (!isAutomationTrigger(definition.trigger) || !isAutomationStatus(definition.status)) {
		return undefined;
	}

	return {
		name: definition.name,
		trigger: definition.trigger,
		status: definition.status
	};
}

export function isProjectMaintenanceDefinition(definition: AutomationDefinitionDto): boolean {
	return (
		definition.recipe === AUTOMATION_RECIPE_ID &&
		definition.version === AUTOMATION_RECIPE_VERSION
	);
}

export function isActiveAutomationRun(
	run: Pick<AutomationRunDto, 'status'> & {
		job: Pick<AutomationRunDto['job'], 'status'>;
	}
): boolean {
	return (
		run.status === 'queued' ||
		run.status === 'running' ||
		run.job.status === 'queued' ||
		run.job.status === 'running'
	);
}

export function formatTimestamp(value: string | null | undefined): string {
	if (!value) return '—';
	const timestamp = new Date(value);
	if (Number.isNaN(timestamp.getTime())) return value;
	return new Intl.DateTimeFormat('en-US', {
		dateStyle: 'medium',
		timeStyle: 'short'
	}).format(timestamp);
}

export function formatInteger(value: number): string {
	return Number.isFinite(value) ? new Intl.NumberFormat('en-US').format(value) : '—';
}

export function formatCostMicros(value: number): string {
	if (!Number.isFinite(value)) return '—';
	const dollars = value / 1_000_000;
	return `$${dollars.toFixed(6)} · ${formatInteger(value)} micros`;
}

function humanize(value: string): string {
	if (!value) return 'Unknown';
	return value.replaceAll('_', ' ').replace(/\b\w/g, (character) => character.toUpperCase());
}
