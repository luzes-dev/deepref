import {
	createWorkflowConnectionId,
	createWorkflowId,
	createWorkflowNodeId,
	createWorkflowNodeKind,
	createWorkflowPortId,
	isJsonObject,
	isJsonValue,
	type JsonObject,
	type JsonValue,
	type WorkflowDefinition,
	type WorkflowNode,
	type WorkflowNodeKind,
	type WorkflowValidationIssue
} from './types';
import { DEFAULT_WORKFLOW_REGISTRY, type WorkflowNodeRegistry } from './registry';
import { validateWorkflow } from './validation';

export interface LegacyAutomationGraphNode {
	id: string;
	type?: string;
	position: { x: number; y: number };
	data: {
		kind?: string;
		label?: string;
		key?: string;
		description?: string;
		config?: unknown;
		[key: string]: unknown;
	};
}

export interface LegacyAutomationGraphEdge {
	id: string;
	source: string;
	target: string;
	sourceHandle?: string | null;
	targetHandle?: string | null;
	type?: string;
	label?: string;
	animated?: boolean;
	style?: string;
	[key: string]: unknown;
}

export interface LegacyAutomationGraphV1 {
	version: 1;
	/** The envelope guard intentionally does not claim node/edge shape. */
	nodes: readonly unknown[];
	edges: readonly unknown[];
	updatedAt?: unknown;
}

export interface LegacyMigrationOptions {
	id?: string;
	name?: string;
	registry?: WorkflowNodeRegistry;
}

export interface WorkflowMigrationSuccess {
	ok: true;
	workflow: WorkflowDefinition;
	fromVersion: 1;
	warnings: readonly string[];
}

export interface WorkflowMigrationFailure {
	ok: false;
	issues: readonly WorkflowValidationIssue[];
	raw?: unknown;
}

export type WorkflowMigrationResult = WorkflowMigrationSuccess | WorkflowMigrationFailure;

export function isLegacyAutomationGraph(value: unknown): value is LegacyAutomationGraphV1 {
	return (
		isJsonObject(value) &&
		value.version === 1 &&
		Array.isArray(value.nodes) &&
		Array.isArray(value.edges)
	);
}

/**
 * Convert the version-one XYFlow-shaped browser draft to the canonical IR.
 * Presentation fields are kept under metadata. This makes the migration
 * lossless while keeping the executable node config free from editor objects.
 */
export function migrateLegacyAutomationGraph(
	value: unknown,
	options: LegacyMigrationOptions = {}
): WorkflowMigrationResult {
	if (!isLegacyAutomationGraph(value)) {
		return {
			ok: false,
			issues: [
				{
					code: 'legacy_migration_failed',
					message: 'Value is not a version-one automation graph.',
					path: 'version'
				}
			],
			raw: value
		};
	}
	if (!isJsonValue(value)) {
		return {
			ok: false,
			issues: [
				{
					code: 'legacy_migration_failed',
					message: 'Legacy automation graph must contain plain JSON data.'
				}
			],
			raw: value
		};
	}

	const issues: WorkflowValidationIssue[] = [];
	const warnings: string[] = [];
	const id = createWorkflowId(options.id ?? 'migrated-workflow');
	if (!id) {
		return {
			ok: false,
			issues: [
				{
					code: 'legacy_migration_failed',
					message: 'Migration needs a stable workflow id.',
					path: 'id'
				}
			],
			raw: value
		};
	}

	const nodes: WorkflowNode[] = [];
	const layout: Record<string, { x: number; y: number }> = {};
	const nodeIds = new Set<string>();
	const nodeKinds = new Map<string, WorkflowNodeKind>();
	for (const [index, valueNode] of value.nodes.entries()) {
		const node = parseLegacyNode(valueNode, index, issues);
		if (!node) continue;
		const nodeId = String(node.node.id);
		if (nodeIds.has(nodeId)) {
			issues.push({
				code: 'duplicate_node_id',
				message: `Duplicate legacy node id: ${nodeId}.`,
				path: `nodes[${index}].id`
			});
			continue;
		}
		nodeIds.add(nodeId);
		nodeKinds.set(nodeId, node.node.kind);
		nodes.push(node.node);
		layout[nodeId] = node.position;
	}

	const connections = [] as WorkflowDefinition['connections'];
	const connectionIds = new Set<string>();
	for (const [index, valueEdge] of value.edges.entries()) {
		const parsed = parseLegacyEdge(valueEdge, index, nodeIds, nodeKinds, issues);
		if (!parsed) continue;
		const connectionId = String(parsed.connection.id);
		if (connectionIds.has(connectionId)) {
			issues.push({
				code: 'duplicate_connection_id',
				message: `Duplicate legacy connection id: ${connectionId}.`,
				path: `edges[${index}].id`
			});
			continue;
		}
		connectionIds.add(connectionId);
		connections.push(parsed.connection);
	}

	if (issues.length > 0) return { ok: false, issues, raw: value };
	const metadata: JsonObject = {
		legacy: {
			format: 'automation-graph-v1',
			...(typeof value.updatedAt === 'string' ? { updatedAt: value.updatedAt } : {})
		}
	};
	const workflow: WorkflowDefinition = {
		schemaVersion: 1,
		id,
		name: options.name?.trim() || 'Migrated automation',
		nodes,
		connections,
		layout: { nodes: layout },
		metadata
	};
	const validation = validateWorkflow(workflow, options.registry ?? DEFAULT_WORKFLOW_REGISTRY);
	if (!validation.valid) return { ok: false, issues: validation.issues, raw: value };
	return { ok: true, workflow, fromVersion: 1, warnings };
}

function parseLegacyNode(
	valueNode: unknown,
	index: number,
	issues: WorkflowValidationIssue[]
): { node: WorkflowNode; position: { x: number; y: number }; presentation: JsonObject } | null {
	if (!isJsonObject(valueNode) || !isJsonObject(valueNode.data)) {
		issues.push({
			code: 'legacy_migration_failed',
			message: 'Legacy node must contain a data object.',
			path: `nodes[${index}]`
		});
		return null;
	}
	const id = createWorkflowNodeId(valueNode.id);
	if (!id) {
		issues.push({
			code: 'invalid_node_id',
			message: 'Legacy node id must be a stable identifier.',
			path: `nodes[${index}].id`
		});
		return null;
	}
	if (
		!isRecord(valueNode.position) ||
		!isFiniteNumber(valueNode.position.x) ||
		!isFiniteNumber(valueNode.position.y)
	) {
		issues.push({
			code: 'legacy_migration_failed',
			message: 'Legacy node position must contain finite x and y.',
			path: `nodes[${index}].position`
		});
		return null;
	}
	const kind = canonicalNodeKind(valueNode.data);
	if (!kind) {
		issues.push({
			code: 'unknown_node_kind',
			message: 'Legacy node has no stable kind or key.',
			path: `nodes[${index}].data`
		});
		return null;
	}
	const config = valueNode.data.config === undefined ? {} : valueNode.data.config;
	if (!isJsonObject(config)) {
		issues.push({
			code: 'invalid_node_config',
			message: 'Legacy node config must be a JSON object.',
			path: `nodes[${index}].data.config`
		});
		return null;
	}
	const presentation = legacyNodePresentation(valueNode.data);
	const node: WorkflowNode = {
		id,
		kind,
		definitionVersion: 1,
		config,
		metadata: presentation
	};
	return { node, position: { x: valueNode.position.x, y: valueNode.position.y }, presentation };
}

function parseLegacyEdge(
	valueEdge: unknown,
	index: number,
	nodeIds: ReadonlySet<string>,
	nodeKinds: ReadonlyMap<string, WorkflowNodeKind>,
	issues: WorkflowValidationIssue[]
): { connection: WorkflowDefinition['connections'][number]; presentation: JsonObject } | null {
	if (!isJsonObject(valueEdge)) {
		issues.push({
			code: 'legacy_migration_failed',
			message: 'Legacy edge must be a JSON object.',
			path: `edges[${index}]`
		});
		return null;
	}
	const id = createWorkflowConnectionId(valueEdge.id);
	const sourceNodeId = createWorkflowNodeId(valueEdge.source);
	const targetNodeId = createWorkflowNodeId(valueEdge.target);
	if (!id || !sourceNodeId || !targetNodeId) {
		issues.push({
			code: 'invalid_connection',
			message: 'Legacy edge id/source/target must be stable identifiers.',
			path: `edges[${index}]`
		});
		return null;
	}
	if (!nodeIds.has(String(sourceNodeId)) || !nodeIds.has(String(targetNodeId))) {
		issues.push({
			code: 'dangling_connection',
			message: 'Legacy edge references a missing node.',
			path: `edges[${index}]`
		});
		return null;
	}
	const sourceHandle = stringOrUndefined(valueEdge.sourceHandle);
	const targetHandle = stringOrUndefined(valueEdge.targetHandle);
	const sourceKind = nodeKinds.get(String(sourceNodeId));
	const targetKind = nodeKinds.get(String(targetNodeId));
	const sourcePort = migrateLegacyPortId(sourceKind, 'output', sourceHandle);
	const targetPort = migrateLegacyPortId(targetKind, 'input', targetHandle);
	const sourcePortId = createWorkflowPortId(sourcePort);
	const targetPortId = createWorkflowPortId(targetPort);
	if (!sourcePortId || !targetPortId) {
		issues.push({
			code: 'invalid_connection',
			message: 'Legacy edge handles must be stable port identifiers.',
			path: `edges[${index}]`
		});
		return null;
	}
	const presentation: JsonObject =
		typeof valueEdge.label === 'string' ? { label: valueEdge.label } : {};
	return {
		connection: {
			id,
			source: { nodeId: sourceNodeId, portId: sourcePortId },
			target: { nodeId: targetNodeId, portId: targetPortId },
			...(Object.keys(presentation).length > 0 ? { metadata: presentation } : {})
		},
		presentation
	};
}

/** Map the single-handle XYFlow draft onto semantic workflow ports. */
function migrateLegacyPortId(
	nodeKind: WorkflowNodeKind | undefined,
	direction: 'input' | 'output',
	handle: string | undefined
): string {
	if (direction === 'output' && nodeKind === 'condition') {
		// The old editor exposed one `output` handle. Preserve that edge by
		// assigning it to the explicit true branch; edges with explicit branch
		// handles retain their branch identity.
		if (handle === undefined || handle === 'output') return 'true';
		return handle;
	}
	if (direction === 'input' && (handle === undefined || handle === 'input')) return 'input';
	return handle ?? (direction === 'output' ? 'output' : 'input');
}

function canonicalNodeKind(data: Record<string, unknown>): WorkflowNodeKind | null {
	const key = typeof data.key === 'string' ? data.key : undefined;
	const category = typeof data.kind === 'string' ? data.kind : undefined;
	const canonical =
		key === 'recompute_project_metrics'
			? key
			: key === 'deterministic_action'
				? key
				: key === 'event_trigger'
					? key
					: key === 'agent_step'
						? key
						: key === 'condition'
							? key
							: key === 'human_gate'
								? key
								: category === 'trigger'
									? 'event_trigger'
									: category === 'action'
										? 'deterministic_action'
										: category === 'agent'
											? 'agent_step'
											: category === 'condition' || category === 'human_gate'
												? category
												: (key ?? category);
	return canonical ? createWorkflowNodeKind(canonical) : null;
}

function legacyNodePresentation(data: JsonObject): JsonObject {
	const presentation: Record<string, JsonValue> = {};
	for (const key of ['label', 'description']) {
		const value = data[key];
		if (typeof value === 'string') presentation[key] = value;
	}
	return presentation;
}

function stringOrUndefined(value: unknown): string | undefined {
	return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function isFiniteNumber(value: unknown): value is number {
	return typeof value === 'number' && Number.isFinite(value);
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}
