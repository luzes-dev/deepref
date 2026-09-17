import { describe, expect, it, vi } from 'vitest';
import { createProjectGraphRenderer } from './project-graph-renderer';

describe('project graph renderer lifecycle', () => {
	it('keeps pre-mount state updates and cleanup safe', async () => {
		const onSelect = vi.fn();
		const onClear = vi.fn();
		const renderer = createProjectGraphRenderer({ onSelect, onClear });

		renderer.setVisibleNodes(new Set(['report-1']));
		renderer.setSelection('report-1');
		renderer.refreshAppearance({ colorBy: 'metrics', fields: ['metrics'] });
		await renderer.reset();
		renderer.clear();
		renderer.destroy();

		expect(onSelect).not.toHaveBeenCalled();
		expect(onClear).not.toHaveBeenCalled();
	});
});
