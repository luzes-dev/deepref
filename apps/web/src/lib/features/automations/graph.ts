/**
 * Compatibility facade for the automation feature's former graph module.
 *
 * Automation drafts are renderer-neutral WorkflowDefinition documents now.
 * Rete belongs to the workflow editor adapter, so this module deliberately
 * has no dependency on a canvas library. A few application callers still use
 * the historic names while the migration settles; those names forward to the
 * canonical automation workflow helpers.
 */

import type { AutomationDraft } from './helpers';
import {
	AUTOMATION_GRAPH_PALETTE,
	automationWorkflowStorageKey,
	cloneWorkflow,
	createDefaultAutomationWorkflow,
	loadAutomationWorkflow,
	saveAutomationWorkflowDraft,
	type AutomationGraphNodeKind,
	type AutomationGraphPaletteItem,
	type AutomationWorkflowLoadResult
} from './components/automation-workflow';
import { deserializeWorkflowDefinition } from '$lib/features/workflows/domain/serialization';
import type {
	JsonObject,
	WorkflowConnection,
	WorkflowDefinition,
	WorkflowNode
} from '$lib/features/workflows/domain/types';

export const AUTOMATION_GRAPH_VERSION = 1 as const;
export const AUTOMATION_GRAPH_STORAGE_PREFIX = 'deepref:automation-graph';

/** The canonical persisted graph document, retained as a compatibility name. */
export type AutomationGraphDraft = WorkflowDefinition;
export type AutomationGraphNode = WorkflowNode;
export type AutomationGraphEdge = WorkflowConnection;

/**
 * Data retained by the superseded presentation component. The active editor
 * consumes WorkflowDefinition nodes through the Rete adapter, but keeping
 * this narrow data contract makes the old component type-safe while it is
 * still present in the application tree.
 */
export interface AutomationGraphNodeData extends Record<string, unknown> {
	kind: AutomationGraphNodeKind;
	label: string;
	key: string;
	description?: string;
	config: JsonObject;
}

export type { AutomationGraphNodeKind, AutomationGraphPaletteItem, AutomationWorkflowLoadResult };
export { AUTOMATION_GRAPH_PALETTE };

export interface AutomationEditorSavePayload {
	projectId: string;
	draft: AutomationDraft;
	graph: WorkflowDefinition;
	definitionId?: string | null;
}

export interface AutomationGraphLoadResult {
	available: boolean;
	draft: WorkflowDefinition | null;
	/** The exact raw value is exposed when a caller needs a recovery affordance. */
	raw?: string | null;
	/** Invalid/future drafts must not be silently replaced. */
	protectedDraft?: boolean;
	error?: string | null;
}

/**
 * Transitional fixture constructor for callers that still need a graph
 * default. New application code should call createDefaultAutomationWorkflow
 * with the real project and definition identity.
 */
export function createDefaultAutomationGraph(draft: AutomationDraft): WorkflowDefinition {
	const workflow = createDefaultAutomationWorkflow({
		draft,
		projectId: 'legacy',
		identity: 'new',
		name: draft.name
	});
	// The legacy helper has no project/definition identity and is used for
	// compatibility callers. Keep its empty-canvas mutation valid; callers
	// with a real identity should use createDefaultAutomationWorkflow, which
	// supplies the editor's initial positions.
	return { ...workflow, layout: { nodes: {} } };
}

export const automationGraphStorageKey = automationWorkflowStorageKey;

export function cloneAutomationGraphDraft(graph: WorkflowDefinition): WorkflowDefinition {
	return cloneWorkflow(graph);
}

/**
 * Read a canonical workflow draft. Legacy version-one XYFlow values are
 * migrated by the shared workflow loader; malformed or future values remain
 * protected and are returned with their raw text.
 */
export function loadAutomationGraphDraft(
	key: string,
	options: {
		draft?: AutomationDraft;
		projectId?: string;
		identity?: string;
		name?: string;
	} = {}
): AutomationGraphLoadResult {
	const storage = readStorage(key);
	if (!storage.available) return { available: false, draft: null, raw: null };
	if (storage.raw === null) return { available: true, draft: null, raw: null };

	// Reuse the feature loader when the richer context is available. This keeps
	// legacy migration and corrupt/future-draft protection on one code path.
	if (options.draft && options.projectId && options.identity !== undefined && options.name) {
		const result = loadAutomationWorkflow({
			key,
			draft: options.draft,
			projectId: options.projectId,
			identity: options.identity,
			name: options.name
		});
		return workflowLoadResult(result);
	}

	const result = deserializeWorkflowDefinition(storage.raw, {
		allowLegacyAutomationGraph: true
	});
	if (!result.success) {
		return {
			available: true,
			draft: null,
			raw: storage.raw,
			protectedDraft: true,
			error: `${result.error} The saved draft was left untouched.`
		};
	}
	return { available: true, draft: result.data };
}

/** Persist only the renderer-neutral workflow JSON. */
export function saveAutomationGraphDraft(key: string, graph: WorkflowDefinition): boolean {
	return saveAutomationWorkflowDraft(key, graph);
}

function workflowLoadResult(result: AutomationWorkflowLoadResult): AutomationGraphLoadResult {
	return {
		available: result.available,
		draft: result.restored ? result.workflow : null,
		raw: result.raw,
		protectedDraft: result.protectedDraft,
		error: result.error
	};
}

function readStorage(key: string): { available: boolean; raw: string | null } {
	try {
		return { available: true, raw: globalThis.localStorage.getItem(key) };
	} catch {
		return { available: false, raw: null };
	}
}

/** Structural check for the no-context compatibility loader. */
