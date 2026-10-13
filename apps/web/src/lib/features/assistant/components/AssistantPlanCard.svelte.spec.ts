import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/svelte';
import AssistantPlanCard from './AssistantPlanCard.svelte';
import type { AssistantPlan } from '../chat-api';

const PROJECT_ID = '33438758-42f0-4da2-9c3d-22a2c51f1109';
const FIRST = '0c1d2e3f-0000-4000-8000-000000000001';
const SECOND = '0c1d2e3f-0000-4000-8000-000000000002';

function plan(overrides: Partial<AssistantPlan> = {}): AssistantPlan {
	return {
		id: '0c1d2e3f-0000-4000-8000-0000000000aa',
		project_id: PROJECT_ID,
		conversation_id: '0c1d2e3f-0000-4000-8000-0000000000bb',
		status: 'pending',
		// A model-written summary that does not match what the plan does.
		summary: 'Tidy up duplicates',
		actions: [
			{
				id: 'a1',
				tool: 'screen_reports',
				summary: 'Tidy up duplicates',
				rationale: 'They look off-topic.',
				affected_count: 2,
				executable: true,
				manual: null,
				decision: 'exclude',
				items: [
					{ report_id: FIRST, title: 'Mice and memory', current_status: 'unscreened' },
					{ report_id: SECOND, title: 'Adults with asthma', current_status: 'include' }
				]
			}
		],
		results: null,
		error: null,
		model: 'test-model',
		prompt_version: 'v1',
		created_at: new Date().toISOString(),
		resolved_by: null,
		resolved_at: null,
		...overrides
	};
}

describe('AssistantPlanCard', () => {
	it('lists the records the plan changes, with their status and the target decision', () => {
		render(AssistantPlanCard, {
			projectId: PROJECT_ID,
			plan: plan(),
			onconfirm: vi.fn(),
			oncancel: vi.fn()
		});

		const action = screen.getByTestId('assistant-plan-action');
		// The headline comes from the action's arguments, not the model's summary.
		expect(within(action).getByTestId('assistant-plan-headline')).toHaveTextContent(
			'Exclude 2 records'
		);
		const items = within(action).getAllByTestId('assistant-plan-item');
		expect(items).toHaveLength(2);
		expect(items[0]).toHaveTextContent('Mice and memory');
		expect(items[0]).toHaveTextContent('Unscreened');
		expect(items[1]).toHaveTextContent('Adults with asthma');
		expect(items[1]).toHaveTextContent('Included');
		expect(items[1]).toHaveTextContent('Exclude');
	});

	it('confirms only the records left selected', async () => {
		const onconfirm = vi.fn();
		render(AssistantPlanCard, {
			projectId: PROJECT_ID,
			plan: plan(),
			onconfirm,
			oncancel: vi.fn()
		});

		await fireEvent.click(screen.getByRole('checkbox', { name: 'Adults with asthma' }));
		expect(screen.getByTestId('assistant-plan-headline')).toHaveTextContent(
			'Exclude 1 of 2 records'
		);
		await fireEvent.click(screen.getByTestId('assistant-plan-confirm'));

		expect(onconfirm).toHaveBeenCalledWith([{ action_id: 'a1', report_id: SECOND }]);
	});

	it('cannot confirm once every record is left out', async () => {
		render(AssistantPlanCard, {
			projectId: PROJECT_ID,
			plan: plan(),
			onconfirm: vi.fn(),
			oncancel: vi.fn()
		});

		await fireEvent.click(screen.getByRole('checkbox', { name: 'Mice and memory' }));
		await fireEvent.click(screen.getByRole('checkbox', { name: 'Adults with asthma' }));

		expect(screen.getByTestId('assistant-plan-confirm')).toBeDisabled();
	});
});
