import { describe, expect, it } from 'vitest';
import { validateWorkflow } from '../workflows/domain/validation';
import { assistantPreviewRegistry, createAssistantPreview } from './workflow-preview';

describe('assistant authority preview', () => {
	it.each([true, false])('builds a typed read-only explanation (proposal=%s)', (proposal) => {
		const preview = createAssistantPreview({
			projectId: 'project-1',
			label: 'Inspect evidence',
			description: 'Returns grounded results.',
			proposal
		});
		expect(validateWorkflow(preview, assistantPreviewRegistry)).toEqual({
			valid: true,
			issues: []
		});
		expect(preview.nodes.at(-1)?.kind).toBe(proposal ? 'assistant.review' : 'assistant.result');
		expect(preview.nodes.every((node) => Object.keys(node.config).length === 0)).toBe(true);
		expect(preview.nodes[1].metadata?.label).toBe('Inspect evidence');
	});

	it('keeps identities and layout when tool labels change', () => {
		const options = {
			projectId: 'project-1',
			label: 'First tool',
			description: 'Description',
			proposal: false
		};
		const first = createAssistantPreview(options);
		const next = createAssistantPreview({ ...options, label: 'Another tool' });
		expect(next.nodes.map((node) => node.id)).toEqual(first.nodes.map((node) => node.id));
		expect(next.connections).toEqual(first.connections);
		expect(next.layout).toEqual(first.layout);
	});
});
