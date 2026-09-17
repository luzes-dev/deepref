import { afterEach, describe, expect, it } from 'vitest';
import { DEFAULT_AUTOMATION_DRAFT } from './helpers';
import {
	automationGraphStorageKey,
	createDefaultAutomationGraph,
	loadAutomationGraphDraft,
	saveAutomationGraphDraft
} from './graph';

afterEach(() => localStorage.clear());

describe('automation graph drafts', () => {
	it('restores graph edits without mixing projects or definitions', () => {
		const key = automationGraphStorageKey('project:one', 'definition:one');
		const graph = createDefaultAutomationGraph(DEFAULT_AUTOMATION_DRAFT);
		const edited = {
			...graph,
			nodes: graph.nodes.map((node) => ({ ...node, label: 'Edited step' })),
			layout: {
				nodes: Object.fromEntries(
					graph.nodes.map((node) => [String(node.id), { x: 123, y: 456 }])
				)
			}
		};
		expect(saveAutomationGraphDraft(key, edited)).toBe(true);
		expect(loadAutomationGraphDraft(key).draft).toEqual(edited);
		expect(
			loadAutomationGraphDraft(automationGraphStorageKey('project:two', 'definition:one'))
				.draft
		).toBeNull();
		expect(
			loadAutomationGraphDraft(automationGraphStorageKey('project:one', 'definition:two'))
				.draft
		).toBeNull();
	});

	it('protects malformed and future drafts without crashing the editor', () => {
		const key = automationGraphStorageKey('project', 'definition');
		for (const raw of [
			'{',
			'null',
			JSON.stringify({
				schemaVersion: 2,
				id: 'future',
				name: 'Future',
				nodes: [],
				connections: []
			}),
			JSON.stringify({
				schemaVersion: 1,
				id: 'broken',
				name: 'Broken',
				nodes: [{}],
				connections: []
			})
		]) {
			localStorage.setItem(key, raw);
			const result = loadAutomationGraphDraft(key);
			expect(result.draft).toBeNull();
			expect(result.protectedDraft).toBe(true);
			expect(result.raw).toBe(raw);
		}
	});

	it('retains an intentionally empty canvas after saving and reopening', () => {
		const key = automationGraphStorageKey('project', 'definition');
		const graph = {
			...createDefaultAutomationGraph(DEFAULT_AUTOMATION_DRAFT),
			nodes: [],
			connections: []
		};
		expect(saveAutomationGraphDraft(key, graph)).toBe(true);
		expect(loadAutomationGraphDraft(key)).toEqual({ available: true, draft: graph });
	});

	it('rejects drafts with duplicate identities or disconnected references', () => {
		const key = automationGraphStorageKey('project', 'definition');
		const graph = createDefaultAutomationGraph(DEFAULT_AUTOMATION_DRAFT);
		const duplicateNodes = { ...graph, nodes: [...graph.nodes, ...graph.nodes] };
		const danglingConnections = {
			...graph,
			connections: [
				{
					...graph.connections[0],
					target: { ...graph.connections[0].target, nodeId: 'missing-node' as never }
				}
			]
		};
		for (const invalid of [duplicateNodes, danglingConnections]) {
			localStorage.setItem(key, JSON.stringify(invalid));
			expect(loadAutomationGraphDraft(key).draft).toBeNull();
		}
	});
});
