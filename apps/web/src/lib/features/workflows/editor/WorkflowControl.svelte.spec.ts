import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';

import WorkflowControl from './WorkflowControl.svelte';
import { WorkflowConfigControl } from './rete-types';

describe('WorkflowControl', () => {
	it('renders the configuration label and commits a text edit', async () => {
		const onChange = vi.fn();
		const control = new WorkflowConfigControl({
			key: 'query',
			value: 'evidence review',
			onChange
		});

		render(WorkflowControl, { data: control });

		const input = screen.getByRole('textbox', { name: 'query' });
		expect(screen.getByText('query')).toBeInTheDocument();
		expect(input).toHaveValue('evidence review');

		await fireEvent.input(input, { target: { value: 'systematic review' } });

		expect(onChange).toHaveBeenCalledOnce();
		expect(onChange).toHaveBeenCalledWith('systematic review');
		expect(control.value).toBe('systematic review');
	});

	it('disables read-only controls and ignores programmatic changes', async () => {
		const onChange = vi.fn();
		const control = new WorkflowConfigControl({
			key: 'approved',
			value: false,
			readonly: true,
			onChange
		});

		render(WorkflowControl, { data: control });

		const checkbox = screen.getByRole('checkbox', { name: 'approved' });
		expect(checkbox).toBeDisabled();
		await fireEvent.click(checkbox);
		control.setValue(true);

		expect(onChange).not.toHaveBeenCalled();
		expect(control.value).toBe(false);
	});

	it('keeps pointer gestures inside a control from bubbling to node dragging', async () => {
		const control = new WorkflowConfigControl({
			key: 'limit',
			value: 10,
			onChange: vi.fn()
		});

		const { container } = render(WorkflowControl, { data: control });
		const input = screen.getByRole('spinbutton', { name: 'limit' });
		const parent = container.querySelector('label');
		if (!parent) throw new Error('Workflow control label was not rendered');
		const onParentPointerDown = vi.fn();
		parent.addEventListener('pointerdown', onParentPointerDown);

		await fireEvent.pointerDown(input);

		expect(onParentPointerDown).not.toHaveBeenCalled();
	});
});
