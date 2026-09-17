import { afterEach, describe, expect, it } from 'vitest';
import { DEFAULT_AUTOMATION_DRAFT } from '../helpers';
import {
	AUTOMATION_GRAPH_PALETTE,
	automationWorkflowStorageKey,
	createDefaultAutomationWorkflow,
	createHistory,
	createWorkflowNodeFromPalette,
	loadAutomationWorkflow,
	saveAutomationWorkflowDraft,
	workflowFingerprint
} from './automation-workflow';
import { workflowCommands } from '$lib/features/workflows/domain/commands';

afterEach(() => localStorage.clear());

describe('canonical automation workflow drafts', () => {
	it('saves and restores a renderer-neutral workflow document', () => {
		const key = automationWorkflowStorageKey('project:one', 'definition:one');
		const workflow = createDefaultAutomationWorkflow({
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project:one',
			identity: 'definition:one',
			name: 'Project maintenance'
		});

		expect(saveAutomationWorkflowDraft(key, workflow)).toBe(true);
		const restored = loadAutomationWorkflow({
			key,
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project:one',
			identity: 'definition:one',
			name: 'Project maintenance'
		});

		expect(restored.restored).toBe(true);
		expect(restored.protectedDraft).toBe(false);
		expect(restored.workflow).toEqual(workflow);
		expect(JSON.parse(localStorage.getItem(key) ?? '{}')).toMatchObject({
			schemaVersion: 1,
			id: String(workflow.id),
			name: 'Project maintenance'
		});
	});

	it('keeps malformed and future drafts protected for recovery', () => {
		const key = automationWorkflowStorageKey('project', 'definition');
		for (const raw of [
			'{',
			JSON.stringify({ schemaVersion: 99, nodes: [], connections: [] })
		]) {
			localStorage.setItem(key, raw);
			const result = loadAutomationWorkflow({
				key,
				draft: DEFAULT_AUTOMATION_DRAFT,
				projectId: 'project',
				identity: 'definition',
				name: 'Project maintenance'
			});

			expect(result.protectedDraft).toBe(true);
			expect(result.raw).toBe(raw);
			expect(localStorage.getItem(key)).toBe(raw);
		}
	});

	it('migrates the previous automation graph envelope through the canonical loader', () => {
		const key = automationWorkflowStorageKey('project', 'definition');
		localStorage.setItem(
			key,
			JSON.stringify({
				version: 1,
				nodes: [
					{
						id: 'trigger-1',
						type: 'automation',
						position: { x: 72, y: 180 },
						data: {
							kind: 'trigger',
							key: 'event_trigger',
							label: 'Manual',
							description: 'Starts the draft.',
							config: { trigger: 'manual', status: 'active' }
						}
					},
					{
						id: 'action-1',
						type: 'automation',
						position: { x: 430, y: 180 },
						data: {
							kind: 'action',
							key: 'recompute_project_metrics',
							label: 'Recompute project metrics',
							description: 'Runs maintenance.',
							config: { recipe: 'project_maintenance.v1', executable: true }
						}
					}
				],
				edges: [
					{
						id: 'trigger-1-action-1',
						source: 'trigger-1',
						target: 'action-1',
						sourceHandle: 'output',
						targetHandle: 'input'
					}
				]
			})
		);

		const result = loadAutomationWorkflow({
			key,
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project',
			identity: 'definition',
			name: 'Migrated maintenance'
		});

		expect(result.restored).toBe(true);
		expect(result.protectedDraft).toBe(false);
		expect(result.workflow.schemaVersion).toBe(1);
		expect(result.workflow.nodes.map((node) => String(node.id))).toEqual([
			'trigger-1',
			'action-1'
		]);
		expect(result.workflow.connections).toHaveLength(1);
	});

	it('creates typed palette nodes and records edits in the shared command history', () => {
		const workflow = createDefaultAutomationWorkflow({
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project',
			identity: 'definition',
			name: 'Project maintenance'
		});
		const condition = AUTOMATION_GRAPH_PALETTE.find((item) => item.kind === 'condition');
		if (!condition) throw new Error('Condition palette item is missing');
		const node = createWorkflowNodeFromPalette({
			item: condition,
			id: 'condition-1',
			position: { x: 800, y: 180 },
			draft: DEFAULT_AUTOMATION_DRAFT
		});

		expect(node.kind).toBe('condition');
		expect(node.config).toEqual({ expression: 'confidence >= 0.85' });

		const history = createHistory(workflow);
		const added = history.execute(workflowCommands.addNode(node, { x: 800, y: 180 }));
		expect(added.ok).toBe(true);
		if (!added.ok) return;
		expect(added.workflow.nodes).toHaveLength(3);
		expect(history.canUndo).toBe(true);
		expect(history.undo().ok).toBe(true);
		expect(history.canRedo).toBe(true);
		expect(history.redo().ok).toBe(true);
	});

	it('keeps browser keys scoped and protects invalid stored drafts at load', () => {
		const first = automationWorkflowStorageKey('project:one', 'definition:one');
		const second = automationWorkflowStorageKey('project:one', 'definition:two');
		const otherProject = automationWorkflowStorageKey('project:two', 'definition:one');
		expect(new Set([first, second, otherProject]).size).toBe(3);

		const workflow = createDefaultAutomationWorkflow({
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project:one',
			identity: 'definition:one',
			name: 'Project maintenance'
		});
		localStorage.setItem(first, '{');
		expect(saveAutomationWorkflowDraft(first, workflow)).toBe(true);
		const stored = localStorage.getItem(first);
		expect(stored).not.toBe('{');
		const restored = loadAutomationWorkflow({
			key: first,
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project:one',
			identity: 'definition:one',
			name: 'Project maintenance'
		});
		expect(workflowFingerprint(restored.workflow)).toBe(workflowFingerprint(workflow));

		localStorage.setItem(second, '{');
		const protectedDraft = loadAutomationWorkflow({
			key: second,
			draft: DEFAULT_AUTOMATION_DRAFT,
			projectId: 'project:one',
			identity: 'definition:two',
			name: 'Project maintenance'
		});
		expect(protectedDraft.protectedDraft).toBe(true);
		expect(localStorage.getItem(second)).toBe('{');
	});
});
