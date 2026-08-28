import { describe, expect, it } from 'vitest';
import type { AutomationDefinitionDto, AutomationRunDto } from '$lib/api/generated/models';
import {
	AUTOMATION_RECIPE_ID,
	AUTOMATION_RECIPE_ROUTE,
	AUTOMATION_RECIPE_VERSION,
	AUTOMATION_TRIGGERS,
	formatCostMicros,
	formatTimestamp,
	isActiveAutomationRun,
	isAutomationTrigger,
	isProjectMaintenanceDefinition,
	labelForTrigger
} from './helpers';

describe('automation helpers', () => {
	it('keeps the trigger catalog closed and labels every supported value', () => {
		expect(AUTOMATION_TRIGGERS).toHaveLength(7);
		expect(isAutomationTrigger('manual')).toBe(true);
		expect(isAutomationTrigger('arbitrary')).toBe(false);
		expect(labelForTrigger('full_text_attached')).toBe('Full text attached');
		expect(labelForTrigger('future_trigger')).toBe('Future Trigger');
	});

	it('keeps the DTO recipe identity separate from the configure route key', () => {
		expect(AUTOMATION_RECIPE_ID).toBe('project_maintenance');
		expect(AUTOMATION_RECIPE_VERSION).toBe(1);
		expect(AUTOMATION_RECIPE_ROUTE).toBe('project_maintenance.v1');

		const supported = {
			created_at: '2026-01-01T00:00:00Z',
			id: 'definition-1',
			name: 'Project maintenance',
			project_id: 'project-1',
			recipe: AUTOMATION_RECIPE_ID,
			status: 'active',
			steps: [],
			trigger: 'manual',
			updated_at: '2026-01-01T00:00:00Z',
			version: AUTOMATION_RECIPE_VERSION
		} satisfies AutomationDefinitionDto;
		expect(isProjectMaintenanceDefinition(supported)).toBe(true);
		const unsupportedRecipe = {
			...supported,
			recipe: AUTOMATION_RECIPE_ROUTE
		} satisfies AutomationDefinitionDto;
		expect(isProjectMaintenanceDefinition(unsupportedRecipe)).toBe(false);
		const unsupportedVersion = { ...supported, version: 2 } satisfies AutomationDefinitionDto;
		expect(isProjectMaintenanceDefinition(unsupportedVersion)).toBe(false);
	});

	it('identifies only queued or running runs for polling', () => {
		const run = {
			status: 'completed',
			job: { status: 'running' }
		} satisfies Pick<AutomationRunDto, 'status'> & {
			job: Pick<AutomationRunDto['job'], 'status'>;
		};
		expect(isActiveAutomationRun(run)).toBe(true);
		expect(
			isActiveAutomationRun({
				...run,
				job: { status: 'completed' }
			})
		).toBe(false);
	});

	it('formats cost and timestamps for an audit-friendly display', () => {
		expect(formatCostMicros(123456)).toBe('$0.123456 · 123,456 micros');
		expect(formatTimestamp('2026-01-02T03:04:00Z')).toContain('Jan 2, 2026');
		expect(formatTimestamp(null)).toBe('—');
	});
});
