import {
	createWorkflowConnectionId,
	createWorkflowId,
	createWorkflowNodeId,
	createWorkflowNodeKind,
	createWorkflowPortId,
	isJsonObject,
	WORKFLOW_SCHEMA_VERSION,
	type WorkflowConnection,
	type WorkflowEndpoint,
	type WorkflowNode,
	type WorkflowNodeId,
	type WorkflowNodeDefinition,
	type WorkflowPortDefinition,
	type WorkflowValidationCode,
	type WorkflowValidationIssue,
	type WorkflowValidationResult
} from './types';
import { DEFAULT_WORKFLOW_REGISTRY, type WorkflowNodeRegistry } from './registry';

// Keep the issue type available from the validation boundary as well as the
// vocabulary module.  Callers should not need to know where the shared issue
// shape is declared in order to render validation failures.
export type { WorkflowValidationIssue } from './types';

export interface ConnectionValidationOptions {
	registry?: WorkflowNodeRegistry;
}

/** Stable tuple encoding avoids collisions when ids contain punctuation. */
export function workflowEndpointKey(endpoint: WorkflowEndpoint): string {
	return JSON.stringify([String(endpoint.nodeId), String(endpoint.portId)]);
}

export function workflowConnectionEndpointKey(connection: WorkflowConnection): string {
	return JSON.stringify([
		String(connection.source.nodeId),
		String(connection.source.portId),
		String(connection.target.nodeId),
		String(connection.target.portId)
	]);
}

export function canConnect(
	sourcePort: WorkflowPortDefinition,
	targetPort: WorkflowPortDefinition,
	options: ConnectionValidationOptions = {}
): boolean {
	return validatePortCompatibility(sourcePort, targetPort, options).valid;
}

export function validatePortCompatibility(
	sourcePort: WorkflowPortDefinition,
	targetPort: WorkflowPortDefinition,
	options: ConnectionValidationOptions = {}
): WorkflowValidationResult {
	const registry = options.registry ?? DEFAULT_WORKFLOW_REGISTRY;
	const issues: WorkflowValidationIssue[] = [];
	if (sourcePort.direction !== 'output') {
		issues.push(issue('invalid_port_direction', 'A connection source must be an output port.'));
	}
	if (targetPort.direction !== 'input') {
		issues.push(issue('invalid_port_direction', 'A connection target must be an input port.'));
	}
	if (!registry.hasDataType(String(sourcePort.dataType))) {
		issues.push(
			issue('unknown_data_type', `Unknown source data type: ${String(sourcePort.dataType)}.`)
		);
	}
	if (!registry.hasDataType(String(targetPort.dataType))) {
		issues.push(
			issue('unknown_data_type', `Unknown target data type: ${String(targetPort.dataType)}.`)
		);
	}
	if (
		issues.length === 0 &&
		!isPortTypeCompatible(sourcePort.dataType, targetPort.dataType, registry)
	) {
		issues.push(
			issue(
				'incompatible_connection',
				`Cannot connect incompatible types: ${String(sourcePort.dataType)} to ${String(targetPort.dataType)}.`
			)
		);
	}
	return issues.length === 0 ? { valid: true, issues: [] } : { valid: false, issues };
}

/**
 * Port compatibility is intentionally independent from a workflow's current
 * connections. Cardinality and duplicate checks belong to validateConnection
 * / validateWorkflow where the surrounding graph is available.
 */
export function isPortTypeCompatible(
	sourceType: string,
	targetType: string,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): boolean {
	if (!registry.hasDataType(sourceType) || !registry.hasDataType(targetType)) return false;
	return sourceType === targetType;
}

export function validateWorkflow(
	value: unknown,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowValidationResult {
	const issues: WorkflowValidationIssue[] = [];
	if (!isJsonObject(value)) {
		return {
			valid: false,
			issues: [issue('invalid_workflow', 'Workflow definition must be a JSON object.')]
		};
	}

	if (value.schemaVersion !== WORKFLOW_SCHEMA_VERSION) {
		issues.push(
			issue(
				value.schemaVersion && typeof value.schemaVersion === 'number'
					? 'unsupported_schema_version'
					: 'invalid_schema_version',
				`Workflow schema version must be ${WORKFLOW_SCHEMA_VERSION}.`
			)
		);
	}
	if (!createWorkflowId(value.id)) {
		issues.push(
			issue('invalid_workflow_id', 'Workflow id must be a stable non-empty identifier.', 'id')
		);
	}
	if (typeof value.name !== 'string' || value.name.trim().length === 0) {
		issues.push(
			issue('invalid_workflow_name', 'Workflow name must be a non-empty string.', 'name')
		);
	}
	if (value.description !== undefined && typeof value.description !== 'string') {
		issues.push(
			issue('invalid_workflow', 'Workflow description must be a string.', 'description')
		);
	}
	if (!Array.isArray(value.nodes)) {
		issues.push(issue('invalid_workflow', 'Workflow nodes must be an array.', 'nodes'));
	}
	if (!Array.isArray(value.connections)) {
		issues.push(
			issue('invalid_workflow', 'Workflow connections must be an array.', 'connections')
		);
	}
	if (value.layout !== undefined) validateLayout(value.layout, value.nodes, issues);
	if (value.metadata !== undefined && !isJsonObject(value.metadata)) {
		issues.push(
			issue('invalid_workflow', 'Workflow metadata must be a JSON object.', 'metadata')
		);
	}

	const nodesById = new Map<string, WorkflowNode>();
	if (Array.isArray(value.nodes)) {
		for (const [index, nodeValue] of value.nodes.entries()) {
			const node = validateNode(nodeValue, index, registry, issues);
			if (!node) continue;
			const key = String(node.id);
			if (nodesById.has(key)) {
				issues.push(
					issue(
						'duplicate_node_id',
						`Duplicate workflow node id: ${key}.`,
						`nodes[${index}].id`
					)
				);
				continue;
			}
			nodesById.set(key, node);
		}
	}

	const connectionIds = new Set<string>();
	const endpointPairs = new Set<string>();
	const connectedInputs = new Set<string>();
	if (Array.isArray(value.connections)) {
		for (const [index, connectionValue] of value.connections.entries()) {
			const connection = validateConnectionShape(connectionValue, index, issues);
			if (!connection) continue;
			const connectionId = String(connection.id);
			if (connectionIds.has(connectionId)) {
				issues.push(
					issue(
						'duplicate_connection_id',
						`Duplicate workflow connection id: ${connectionId}.`,
						`connections[${index}].id`
					)
				);
			}
			connectionIds.add(connectionId);

			const source = nodesById.get(String(connection.source.nodeId));
			const target = nodesById.get(String(connection.target.nodeId));
			if (!source || !target) {
				issues.push(
					issue(
						'dangling_connection',
						`Connection ${connectionId} references a missing node.`,
						`connections[${index}]`
					)
				);
				continue;
			}
			if (connection.source.nodeId === connection.target.nodeId) {
				issues.push(
					issue(
						'self_connection',
						`Connection ${connectionId} cannot connect a node to itself.`,
						`connections[${index}]`
					)
				);
				continue;
			}

			const sourceDefinition = registry.get(source.kind, source.definitionVersion);
			const targetDefinition = registry.get(target.kind, target.definitionVersion);
			if (!sourceDefinition || !targetDefinition) continue;
			const sourcePort = sourceDefinition.outputs.find(
				(port) => String(port.id) === String(connection.source.portId)
			);
			const targetPort = targetDefinition.inputs.find(
				(port) => String(port.id) === String(connection.target.portId)
			);
			if (!sourcePort || !targetPort) {
				issues.push(
					issue(
						'unknown_port',
						`Connection ${connectionId} references an unknown port.`,
						`connections[${index}]`
					)
				);
				continue;
			}

			const compatibility = validatePortCompatibility(sourcePort, targetPort, { registry });
			if (!compatibility.valid) {
				issues.push(
					...compatibility.issues.map((value) => ({
						...value,
						path: value.path ?? `connections[${index}]`
					}))
				);
			}

			const pair = workflowConnectionEndpointKey(connection);
			if (endpointPairs.has(pair)) {
				issues.push(
					issue(
						'duplicate_connection',
						`Duplicate connection endpoints: ${pair}.`,
						`connections[${index}]`
					)
				);
			}
			endpointPairs.add(pair);
			if (targetPort.cardinality !== 'many') {
				const targetKey = workflowEndpointKey(connection.target);
				if (connectedInputs.has(targetKey)) {
					issues.push(
						issue(
							'input_already_connected',
							`Input port ${targetKey} accepts only one connection.`,
							`connections[${index}]`
						)
					);
				}
				connectedInputs.add(targetKey);
			}
		}
	}

	return issues.length === 0 ? { valid: true, issues: [] } : { valid: false, issues };
}

export function validateConnection(
	value: unknown,
	workflow: unknown,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowValidationResult {
	if (!isJsonObject(workflow) || !Array.isArray(workflow.nodes)) {
		return {
			valid: false,
			issues: [issue('invalid_workflow', 'Workflow nodes must be an array.', 'nodes')]
		};
	}
	if (!isJsonObject(value)) {
		return {
			valid: false,
			issues: [issue('invalid_connection', 'Workflow connection must be a JSON object.')]
		};
	}
	const nodeById = new Map<string, WorkflowNode>();
	for (const nodeValue of workflow.nodes) {
		if (!isRecord(nodeValue)) continue;
		const id = createWorkflowNodeId(nodeValue.id);
		const kind = createWorkflowNodeKind(nodeValue.kind);
		const definitionVersion = positiveInteger(nodeValue.definitionVersion);
		if (id && kind && definitionVersion !== null && isJsonObject(nodeValue.config)) {
			nodeById.set(String(id), {
				id,
				kind,
				definitionVersion,
				config: nodeValue.config
			});
		}
	}
	const shapeIssues: WorkflowValidationIssue[] = [];
	const shape = validateConnectionShape(value, 0, shapeIssues);
	if (!shape || shapeIssues.length > 0)
		return {
			valid: false,
			issues:
				shapeIssues.length > 0
					? shapeIssues
					: [issue('invalid_connection', 'Invalid workflow connection.')]
		};
	const source = nodeById.get(String(shape.source.nodeId));
	const target = nodeById.get(String(shape.target.nodeId));
	if (!source || !target) {
		return {
			valid: false,
			issues: [issue('dangling_connection', 'Connection references a missing node.')]
		};
	}
	if (shape.source.nodeId === shape.target.nodeId) {
		return {
			valid: false,
			issues: [issue('self_connection', 'A workflow node cannot connect to itself.')]
		};
	}
	const sourceDefinition = registry.get(source.kind, source.definitionVersion);
	const targetDefinition = registry.get(target.kind, target.definitionVersion);
	if (!sourceDefinition || !targetDefinition) {
		return {
			valid: false,
			issues: [
				issue(
					registry.has(source.kind) && registry.has(target.kind)
						? 'unsupported_node_definition_version'
						: 'unknown_node_kind',
					'Connection references an unsupported node definition.'
				)
			]
		};
	}
	const sourcePort = sourceDefinition.outputs.find(
		(port) => String(port.id) === String(shape.source.portId)
	);
	const targetPort = targetDefinition.inputs.find(
		(port) => String(port.id) === String(shape.target.portId)
	);
	if (!sourcePort || !targetPort) {
		return {
			valid: false,
			issues: [issue('unknown_port', 'Connection references an unknown port.')]
		};
	}
	const compatibility = validatePortCompatibility(sourcePort, targetPort, {
		registry
	});
	if (!compatibility.valid) return compatibility;
	if (Array.isArray(workflow.connections)) {
		for (const candidate of workflow.connections) {
			if (!isJsonObject(candidate)) continue;
			const candidateId = createWorkflowConnectionId(candidate.id);
			if (candidateId && candidateId === shape.id) {
				return {
					valid: false,
					issues: [issue('duplicate_connection_id', 'Connection id is already in use.')]
				};
			}
			const candidateSource = isJsonObject(candidate.source)
				? {
						nodeId: createWorkflowNodeId(candidate.source.nodeId),
						portId: createWorkflowPortId(candidate.source.portId)
					}
				: undefined;
			const candidateTarget = isJsonObject(candidate.target)
				? {
						nodeId: createWorkflowNodeId(candidate.target.nodeId),
						portId: createWorkflowPortId(candidate.target.portId)
					}
				: undefined;
			if (
				candidateSource?.nodeId &&
				candidateSource.portId &&
				candidateTarget?.nodeId &&
				candidateTarget.portId &&
				workflowConnectionEndpointKey({
					id: shape.id,
					source: {
						nodeId: candidateSource.nodeId,
						portId: candidateSource.portId
					},
					target: {
						nodeId: candidateTarget.nodeId,
						portId: candidateTarget.portId
					}
				}) === workflowConnectionEndpointKey(shape)
			) {
				return {
					valid: false,
					issues: [
						issue('duplicate_connection', 'Connection endpoints are already in use.')
					]
				};
			}
		}
	}
	if (targetPort.cardinality !== 'many' && Array.isArray(workflow.connections)) {
		const targetKey = workflowEndpointKey(shape.target);
		const occupied = workflow.connections.some((candidate) => {
			if (!isJsonObject(candidate) || !isJsonObject(candidate.target)) return false;
			const candidateNodeId = createWorkflowNodeId(candidate.target.nodeId);
			const candidatePortId = createWorkflowPortId(candidate.target.portId);
			return (
				candidateNodeId !== null &&
				candidatePortId !== null &&
				workflowEndpointKey({ nodeId: candidateNodeId, portId: candidatePortId }) ===
					targetKey
			);
		});
		if (occupied) {
			return {
				valid: false,
				issues: [
					issue(
						'input_already_connected',
						`Input port ${String(shape.target.nodeId)}:${String(shape.target.portId)} accepts only one connection.`
					)
				]
			};
		}
	}
	return compatibility;
}

export interface CycleDetectionResult {
	hasCycles: boolean;
	cycles: readonly (readonly WorkflowNodeId[])[];
}

/** Detect directed cycles without making the persisted IR DAG-only. */
export function detectCycles(
	nodes: readonly WorkflowNode[],
	connections: readonly WorkflowConnection[]
): CycleDetectionResult {
	const adjacency = new Map<string, WorkflowNodeId[]>();
	for (const node of nodes) adjacency.set(String(node.id), []);
	for (const connection of connections) {
		const neighbors = adjacency.get(String(connection.source.nodeId));
		if (neighbors && adjacency.has(String(connection.target.nodeId))) {
			neighbors.push(connection.target.nodeId);
		}
	}

	const visited = new Set<string>();
	const active = new Set<string>();
	const path: WorkflowNodeId[] = [];
	const cycles: WorkflowNodeId[][] = [];

	const visit = (nodeId: WorkflowNodeId): void => {
		const key = String(nodeId);
		visited.add(key);
		active.add(key);
		path.push(nodeId);
		for (const next of adjacency.get(key) ?? []) {
			const nextKey = String(next);
			if (!visited.has(nextKey)) {
				visit(next);
			} else if (active.has(nextKey)) {
				const start = path.findIndex((value) => String(value) === nextKey);
				if (start >= 0) cycles.push([...path.slice(start), next]);
			}
		}
		path.pop();
		active.delete(key);
	};

	for (const node of nodes) {
		if (!visited.has(String(node.id))) visit(node.id);
	}
	return { hasCycles: cycles.length > 0, cycles };
}

function validateNode(
	value: unknown,
	index: number,
	registry: WorkflowNodeRegistry,
	issues: WorkflowValidationIssue[]
): WorkflowNode | null {
	if (!isRecord(value)) {
		issues.push(
			issue('invalid_node', 'Workflow node must be a JSON object.', `nodes[${index}]`)
		);
		return null;
	}
	const id = createWorkflowNodeId(value.id);
	if (!id)
		issues.push(
			issue(
				'invalid_node_id',
				'Node id must be a stable non-empty identifier.',
				`nodes[${index}].id`
			)
		);
	const kind = createWorkflowNodeKind(value.kind);
	if (!kind)
		issues.push(
			issue(
				'unknown_node_kind',
				'Node kind must be a stable identifier.',
				`nodes[${index}].kind`
			)
		);
	const definitionVersion = positiveInteger(value.definitionVersion);
	if (definitionVersion === null) {
		issues.push(
			issue(
				'unsupported_node_definition_version',
				'Node definitionVersion must be a positive integer.',
				`nodes[${index}].definitionVersion`
			)
		);
	}
	if (!isJsonObject(value.config)) {
		issues.push(
			issue(
				'invalid_node_config',
				'Node config must be a JSON object.',
				`nodes[${index}].config`
			)
		);
	}
	if (value.metadata !== undefined && !isJsonObject(value.metadata)) {
		issues.push(
			issue(
				'invalid_node',
				'Node metadata must be a JSON object.',
				`nodes[${index}].metadata`
			)
		);
	}
	if (value.label !== undefined && typeof value.label !== 'string') {
		issues.push(issue('invalid_node', 'Node label must be a string.', `nodes[${index}].label`));
	}
	if (!id || !kind || definitionVersion === null || !isJsonObject(value.config)) {
		return null;
	}

	const definition = registry.get(kind, definitionVersion);
	if (!definition) {
		issues.push(
			issue(
				registry.has(kind) ? 'unsupported_node_definition_version' : 'unknown_node_kind',
				`No node definition is registered for ${String(kind)}@${definitionVersion}.`,
				`nodes[${index}]`
			)
		);
		return null;
	}
	const configResult = safeValidateConfig(definition, value.config);
	if (!configResult.valid) {
		issues.push(
			...configResult.issues.map((configIssue) => ({
				...configIssue,
				code:
					configIssue.code === 'invalid_node_config'
						? configIssue.code
						: 'invalid_node_config',
				path: configIssue.path ?? `nodes[${index}].config`
			}))
		);
	}
	return {
		id,
		kind,
		definitionVersion,
		config: value.config,
		label: typeof value.label === 'string' ? value.label : undefined,
		metadata: isJsonObject(value.metadata) ? value.metadata : undefined
	};
}

function validateConnectionShape(
	value: unknown,
	index: number,
	issues: WorkflowValidationIssue[]
): WorkflowConnection | null {
	if (!isRecord(value)) {
		issues.push(
			issue(
				'invalid_connection',
				'Workflow connection must be a JSON object.',
				`connections[${index}]`
			)
		);
		return null;
	}
	const id = createWorkflowConnectionId(value.id);
	if (!id)
		issues.push(
			issue(
				'invalid_connection_id',
				'Connection id must be a stable non-empty identifier.',
				`connections[${index}].id`
			)
		);
	const source = parseEndpoint(value.source, `connections[${index}].source`, issues);
	const target = parseEndpoint(value.target, `connections[${index}].target`, issues);
	if (!id || !source || !target) return null;
	if (value.metadata !== undefined && !isJsonObject(value.metadata)) {
		issues.push(
			issue(
				'invalid_connection',
				'Connection metadata must be a JSON object.',
				`connections[${index}].metadata`
			)
		);
	}
	return {
		id,
		source,
		target,
		metadata: isJsonObject(value.metadata) ? value.metadata : undefined
	};
}

function parseEndpoint(
	value: unknown,
	path: string,
	issues: WorkflowValidationIssue[]
): WorkflowEndpoint | null {
	if (!isRecord(value)) {
		issues.push(
			issue('invalid_connection', 'Connection endpoint must be a JSON object.', path)
		);
		return null;
	}
	const nodeId = createWorkflowNodeId(value.nodeId);
	const portId = createWorkflowPortId(value.portId);
	if (!nodeId)
		issues.push(
			issue(
				'invalid_connection',
				'Endpoint nodeId must be a stable identifier.',
				`${path}.nodeId`
			)
		);
	if (!portId)
		issues.push(
			issue(
				'invalid_connection',
				'Endpoint portId must be a stable identifier.',
				`${path}.portId`
			)
		);
	return nodeId && portId ? { nodeId, portId } : null;
}

function validateLayout(value: unknown, nodes: unknown, issues: WorkflowValidationIssue[]): void {
	if (!isRecord(value) || !isRecord(value.nodes)) {
		issues.push(
			issue('invalid_layout', 'Workflow layout must contain a nodes object.', 'layout')
		);
		return;
	}
	const nodeIds = new Set(
		Array.isArray(nodes)
			? nodes
					.map((node) => (isRecord(node) ? createWorkflowNodeId(node.id) : null))
					.filter((id): id is NonNullable<typeof id> => id !== null)
					.map(String)
			: []
	);
	for (const [nodeId, layout] of Object.entries(value.nodes)) {
		if (!nodeIds.has(nodeId)) {
			issues.push(
				issue(
					'unknown_layout_node',
					`Layout references missing node ${nodeId}.`,
					`layout.nodes.${nodeId}`
				)
			);
		}
		if (!isRecord(layout) || !isFiniteNumber(layout.x) || !isFiniteNumber(layout.y)) {
			issues.push(
				issue(
					'invalid_layout',
					`Layout for ${nodeId} must contain finite x and y.`,
					`layout.nodes.${nodeId}`
				)
			);
			continue;
		}
		for (const key of ['width', 'height']) {
			const dimension = layout[key];
			if (dimension !== undefined && (!isFiniteNumber(dimension) || dimension <= 0)) {
				issues.push(
					issue(
						'invalid_layout',
						`Layout ${key} for ${nodeId} must be positive.`,
						`layout.nodes.${nodeId}.${key}`
					)
				);
			}
		}
	}
}

function safeValidateConfig(
	definition: WorkflowNodeDefinition,
	config: unknown
): WorkflowValidationResult {
	try {
		return definition.validateConfig?.(config) ?? { valid: true, issues: [] };
	} catch {
		return {
			valid: false,
			issues: [
				issue(
					'invalid_node_config',
					`Configuration validation failed for ${String(definition.kind)}@${definition.version}.`
				)
			]
		};
	}
}

function issue(
	code: WorkflowValidationCode,
	message: string,
	path?: string
): WorkflowValidationIssue {
	return { code, message, path };
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function isFiniteNumber(value: unknown): value is number {
	return typeof value === 'number' && Number.isFinite(value);
}

function positiveInteger(value: unknown): number | null {
	return typeof value === 'number' && Number.isInteger(value) && value >= 1 ? value : null;
}
