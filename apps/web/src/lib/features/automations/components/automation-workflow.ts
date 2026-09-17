import { AUTOMATION_RECIPE_ROUTE, labelForTrigger, type AutomationDraft } from '../helpers';
import type { PredefinedRecipe } from '../recipes';
export * from '../recipes';
import {
	createWorkflowHistory,
	WorkflowHistory,
	type WorkflowCommand
} from '$lib/features/workflows/domain/commands';
import {
	DEFAULT_WORKFLOW_REGISTRY,
	type WorkflowNodeRegistry
} from '$lib/features/workflows/domain/registry';
import {
	CURRENT_SCHEMA_VERSION,
	deserializeWorkflowDefinition,
	serializeWorkflowDefinition
} from '$lib/features/workflows/domain/serialization';
import { validateWorkflow } from '$lib/features/workflows/domain/validation';
import {
	WORKFLOW_SCHEMA_VERSION,
	createWorkflowNodeId,
	createWorkflowNodeKind,
	isJsonObject,
	workflowConnectionId,
	workflowId,
	workflowNodeId,
	workflowNodeKind,
	workflowPortId,
	type JsonObject,
	type WorkflowDefinition,
	type WorkflowNode,
	type WorkflowNodeKind
} from '$lib/features/workflows/domain/types';

/**
 * The automation feature owns the browser key namespace. The value stored at
 * that key is the canonical workflow JSON; no renderer object is persisted.
 */
export const AUTOMATION_GRAPH_STORAGE_PREFIX = 'deepref:automation-graph';
export const AUTOMATION_WORKFLOW_SCHEMA_VERSION = WORKFLOW_SCHEMA_VERSION;

export type AutomationGraphNodeKind = 'trigger' | 'action' | 'condition' | 'agent' | 'human_gate';

export interface AutomationGraphPaletteItem {
	readonly kind: AutomationGraphNodeKind;
	readonly label: string;
	readonly key: string;
	readonly description: string;
}

export const AUTOMATION_GRAPH_PALETTE = [
	{
		kind: 'trigger',
		label: 'Trigger',
		key: 'event_trigger',
		description: 'Start the draft from a project event or manual dispatch.'
	},
	{
		kind: 'action',
		label: 'Action',
		key: 'deterministic_action',
		description: 'Represent a deterministic project operation.'
	},
	{
		kind: 'condition',
		label: 'Condition',
		key: 'condition',
		description: 'Branch the graph on a deterministic expression.'
	},
	{
		kind: 'agent',
		label: 'Agent step',
		key: 'agent_step',
		description: 'Sketch an AI-assisted analysis step.'
	},
	{
		kind: 'human_gate',
		label: 'Human gate',
		key: 'human_gate',
		description: 'Require reviewer approval before subsequent steps proceed.'
	}
] as const satisfies readonly AutomationGraphPaletteItem[];

export interface AutomationWorkflowLoadResult {
	readonly available: boolean;
	readonly workflow: WorkflowDefinition;
	readonly restored: boolean;
	readonly error: string | null;
	readonly warnings: readonly string[];
	readonly protectedDraft: boolean;
	readonly raw: string | null;
}

export function automationWorkflowStorageKey(projectId: string, identity: string): string {
	return `${AUTOMATION_GRAPH_STORAGE_PREFIX}:${projectId}:${identity}`;
}

export function loadAutomationWorkflow({
	key,
	draft,
	projectId,
	identity,
	name,
	registry = DEFAULT_WORKFLOW_REGISTRY
}: {
	readonly key: string;
	readonly draft: AutomationDraft;
	readonly projectId: string;
	readonly identity: string;
	readonly name: string;
	readonly registry?: WorkflowNodeRegistry;
}): AutomationWorkflowLoadResult {
	const fallback = createDefaultAutomationWorkflow({
		draft,
		projectId,
		identity,
		name,
		registry
	});
	const storage = readStorageValue(key);
	if (!storage.available) {
		return {
			available: false,
			workflow: fallback,
			restored: false,
			error: null,
			warnings: [],
			protectedDraft: false,
			raw: null
		};
	}
	if (storage.raw === null) {
		return {
			available: true,
			workflow: fallback,
			restored: false,
			error: null,
			warnings: [],
			protectedDraft: false,
			raw: null
		};
	}

	const futureVersion = futureDraftVersion(storage.raw);
	if (futureVersion !== null) {
		return protectedLoadResult(
			fallback,
			`This graph draft uses schema version ${futureVersion}, which this editor cannot open yet. It was left untouched.`,
			storage.raw
		);
	}

	const result = deserializeWorkflowDefinition(storage.raw, {
		registry,
		allowLegacyAutomationGraph: true,
		legacy: {
			id: workflowIdentity(projectId, identity),
			name,
			registry
		}
	});
	if (!result.success) {
		return protectedLoadResult(
			fallback,
			`The saved graph draft could not be opened: ${result.error} It was left untouched.`,
			storage.raw
		);
	}

	return {
		available: true,
		workflow: cloneWorkflow(result.data),
		restored: true,
		error: null,
		warnings: result.warnings ?? [],
		protectedDraft: false,
		raw: storage.raw
	};
}

export function saveAutomationWorkflowDraft(
	key: string,
	workflow: WorkflowDefinition,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): boolean {
	try {
		const storage = globalThis.localStorage;
		storage.setItem(key, serializeWorkflowDefinition(workflow, { registry }));
		return true;
	} catch {
		return false;
	}
}

export function workflowIdentity(projectId: string, identity: string): string {
	const project = stableIdentityPart(projectId);
	const definition = stableIdentityPart(identity || 'new');
	return `automation-${project}-${definition}`;
}

export function createDefaultAutomationWorkflow({
	draft,
	projectId,
	identity,
	name,
	registry = DEFAULT_WORKFLOW_REGISTRY
}: {
	readonly draft: AutomationDraft;
	readonly projectId: string;
	readonly identity: string;
	readonly name: string;
	readonly registry?: WorkflowNodeRegistry;
}): WorkflowDefinition {
	const triggerId = workflowNodeId('trigger-1');
	const actionId = workflowNodeId('action-1');
	const trigger: WorkflowNode = {
		id: triggerId,
		kind: workflowNodeKind('event_trigger'),
		definitionVersion: 1,
		config: { trigger: draft.trigger, status: draft.status },
		metadata: {
			label: labelForTrigger(draft.trigger),
			description: 'The configured project event opens this draft.'
		}
	};
	const action: WorkflowNode = {
		id: actionId,
		kind: workflowNodeKind('recompute_project_metrics'),
		definitionVersion: 1,
		config: { recipe: AUTOMATION_RECIPE_ROUTE, executable: true },
		metadata: {
			label: 'Recompute project metrics',
			description: 'The only step currently executed by the built-in recipe.'
		}
	};
	const workflow: WorkflowDefinition = {
		schemaVersion: AUTOMATION_WORKFLOW_SCHEMA_VERSION,
		id: workflowId(workflowIdentity(projectId, identity)),
		name: name.trim() || 'Untitled automation',
		nodes: [trigger, action],
		connections: [
			{
				id: workflowConnectionId('trigger-1-action-1'),
				source: { nodeId: triggerId, portId: workflowPortId('output') },
				target: { nodeId: actionId, portId: workflowPortId('input') }
			}
		],
		layout: {
			nodes: {
				[String(triggerId)]: { x: 72, y: 180 },
				[String(actionId)]: { x: 430, y: 180 }
			}
		}
	};
	const validation = validateWorkflow(workflow, registry);
	if (!validation.valid) {
		throw new Error(
			`Default automation workflow is invalid: ${formatIssues(validation.issues)}`
		);
	}
	return workflow;
}

export function createWorkflowFromRecipe({
	recipe,
	projectId,
	identity,
	name,
	registry = DEFAULT_WORKFLOW_REGISTRY
}: {
	readonly recipe: PredefinedRecipe;
	readonly projectId: string;
	readonly identity: string;
	readonly name?: string;
	readonly registry?: WorkflowNodeRegistry;
}): WorkflowDefinition {
	const triggerId = workflowNodeId('trigger-1');
	const actionId = workflowNodeId('action-1');
	const trigger: WorkflowNode = {
		id: triggerId,
		kind: workflowNodeKind('event_trigger'),
		definitionVersion: 1,
		config: { trigger: 'manual', status: 'active' },
		metadata: {
			label: 'Manual Trigger',
			description: `Starts the ${recipe.title} recipe execution.`
		}
	};
	const action: WorkflowNode = {
		id: actionId,
		kind: workflowNodeKind('recompute_project_metrics'),
		definitionVersion: 1,
		config: { recipe: AUTOMATION_RECIPE_ROUTE, executable: true },
		metadata: {
			label: recipe.title,
			description: recipe.description
		}
	};
	const workflow: WorkflowDefinition = {
		schemaVersion: AUTOMATION_WORKFLOW_SCHEMA_VERSION,
		id: workflowId(workflowIdentity(projectId, identity)),
		name: name?.trim() || `${recipe.title} Workflow` || 'Untitled automation',
		nodes: [trigger, action],
		connections: [
			{
				id: workflowConnectionId('trigger-1-action-1'),
				source: { nodeId: triggerId, portId: workflowPortId('output') },
				target: { nodeId: actionId, portId: workflowPortId('input') }
			}
		],
		layout: {
			nodes: {
				[String(triggerId)]: { x: 72, y: 180 },
				[String(actionId)]: { x: 430, y: 180 }
			}
		}
	};
	const validation = validateWorkflow(workflow, registry);
	if (!validation.valid) {
		throw new Error(
			`Recipe automation workflow is invalid: ${formatIssues(validation.issues)}`
		);
	}
	return workflow;
}

export function cloneWorkflow(workflow: WorkflowDefinition): WorkflowDefinition {
	return structuredClone(workflow);
}

export function workflowFingerprint(workflow: WorkflowDefinition): string {
	return JSON.stringify(workflow);
}

export function paletteItemForKind(kind: WorkflowNodeKind): AutomationGraphPaletteItem | undefined {
	const canonical = String(kind);
	return AUTOMATION_GRAPH_PALETTE.find((item) => paletteKind(item) === canonical);
}

export function createWorkflowNodeFromPalette({
	item,
	id,
	position,
	draft
}: {
	readonly item: AutomationGraphPaletteItem;
	readonly id: string;
	readonly position: { readonly x: number; readonly y: number };
	readonly draft: AutomationDraft;
}): WorkflowNode {
	const nodeId = createWorkflowNodeId(id);
	if (!nodeId) throw new TypeError('Generated workflow node id is invalid.');
	const kind = createWorkflowNodeKind(paletteKind(item));
	if (!kind) throw new TypeError(`Unsupported automation palette kind: ${item.key}`);
	return {
		id: nodeId,
		kind,
		definitionVersion: 1,
		config: defaultConfigForPalette(item, draft),
		metadata: {
			label: item.label,
			description: item.description,
			position: { x: position.x, y: position.y }
		}
	};
}

/** Domain history is the only history implementation used by the editor. */
export function createHistory(
	initial: WorkflowDefinition,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowHistory {
	return createWorkflowHistory(initial, { registry });
}

export type { WorkflowCommand };

function protectedLoadResult(
	workflow: WorkflowDefinition,
	error: string,
	raw: string
): AutomationWorkflowLoadResult {
	return {
		available: true,
		workflow,
		restored: false,
		error,
		warnings: [],
		protectedDraft: true,
		raw
	};
}

function readStorageValue(key: string): { available: boolean; raw: string | null } {
	try {
		return { available: true, raw: globalThis.localStorage.getItem(key) };
	} catch {
		return { available: false, raw: null };
	}
}

function futureDraftVersion(raw: string): number | null {
	try {
		const value: unknown = JSON.parse(raw);
		if (!isJsonObject(value)) return null;
		if (
			typeof value.schemaVersion === 'number' &&
			value.schemaVersion > CURRENT_SCHEMA_VERSION
		) {
			return value.schemaVersion;
		}
		if (typeof value.version === 'number' && value.version > 1) return value.version;
		return null;
	} catch {
		return null;
	}
}

function paletteKind(item: AutomationGraphPaletteItem): string {
	switch (item.kind) {
		case 'trigger':
			return 'event_trigger';
		case 'action':
			return item.key === 'recompute_project_metrics'
				? 'recompute_project_metrics'
				: 'deterministic_action';
		case 'condition':
			return 'condition';
		case 'agent':
			return 'agent_step';
		case 'human_gate':
			return 'human_gate';
		default: {
			const exhaustive: never = item.kind;
			return exhaustive;
		}
	}
}

function defaultConfigForPalette(
	item: AutomationGraphPaletteItem,
	draft: AutomationDraft
): JsonObject {
	switch (item.kind) {
		case 'trigger':
			return { trigger: draft.trigger, status: draft.status };
		case 'action':
			return item.key === 'recompute_project_metrics'
				? { recipe: AUTOMATION_RECIPE_ROUTE, executable: true }
				: { operation: item.key };
		case 'condition':
			return { expression: 'confidence >= 0.85' };
		case 'agent':
			return { model: 'review-assistant' };
		case 'human_gate':
			return { approvals: 1 };
		default: {
			const exhaustive: never = item.kind;
			return exhaustive;
		}
	}
}

function stableIdentityPart(value: string): string {
	const normalized = value.trim().replace(/[^A-Za-z0-9._:-]+/g, '-');
	return normalized || 'unknown';
}

function formatIssues(issues: readonly { message: string }[]): string {
	return issues.map((issue) => issue.message).join(' ');
}
