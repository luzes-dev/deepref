import { DEFAULT_WORKFLOW_REGISTRY, type WorkflowNodeRegistry } from './registry';
import { validateConnection, validateWorkflow, type WorkflowValidationIssue } from './validation';
import {
	WORKFLOW_SCHEMA_VERSION,
	isJsonObject,
	workflowConnectionId,
	workflowId,
	workflowNodeId,
	workflowNodeKind,
	type JsonObject,
	type WorkflowConnection,
	type WorkflowDefinition,
	type WorkflowNode,
	type WorkflowNodeId,
	type WorkflowNodeLayout,
	type WorkflowCommandResult
} from './types';

export type WorkflowCommand =
	| {
			type: 'add-node';
			node: WorkflowNode;
			position?: WorkflowNodeLayout;
	  }
	| {
			type: 'remove-node';
			nodeId: WorkflowNodeId | string;
	  }
	| {
			type: 'add-connection';
			connection: WorkflowConnection;
	  }
	| {
			type: 'remove-connection';
			connectionId: string;
	  }
	| {
			type: 'update-node-config';
			nodeId: WorkflowNodeId | string;
			config: JsonObject;
	  }
	| {
			type: 'update-node-position';
			nodeId: WorkflowNodeId | string;
			position: WorkflowNodeLayout;
	  };

export class WorkflowCommandError extends Error {
	readonly issues: readonly WorkflowValidationIssue[];

	constructor(message: string, issues: readonly WorkflowValidationIssue[] = []) {
		super(message);
		this.name = 'WorkflowCommandError';
		this.issues = issues;
	}
}

export function createEmptyWorkflow(
	id: string = `wf-${Date.now()}`,
	name = 'New Research Workflow',
	description?: string
): WorkflowDefinition {
	const workflowIdValue = workflowId(id);
	if (name.trim().length === 0) throw new TypeError('Workflow name must be non-empty');
	return {
		schemaVersion: WORKFLOW_SCHEMA_VERSION,
		id: workflowIdValue,
		name,
		...(description !== undefined ? { description } : {}),
		nodes: [],
		connections: [],
		layout: { nodes: {} }
	};
}

export function addNode(
	workflow: WorkflowDefinition,
	node: WorkflowNode,
	position?: WorkflowNodeLayout,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	if (workflow.nodes.some((candidate) => candidate.id === node.id)) {
		throw new WorkflowCommandError(
			`Node with id '${String(node.id)}' already exists in workflow.`
		);
	}
	const layoutNodes = { ...(workflow.layout?.nodes ?? {}) };
	if (position !== undefined) layoutNodes[String(node.id)] = clonePosition(position);
	return assertValid(
		{
			...cloneWorkflow(workflow),
			nodes: [...workflow.nodes, cloneNode(node)],
			layout: { ...(workflow.layout ?? {}), nodes: layoutNodes }
		},
		registry,
		'Cannot add workflow node.'
	);
}

export function removeNode(
	workflow: WorkflowDefinition,
	nodeId: WorkflowNodeId | string,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	const id = String(nodeId);
	if (!workflow.nodes.some((node) => String(node.id) === id)) {
		throw new WorkflowCommandError(`Workflow node '${id}' does not exist.`);
	}
	const nodes = workflow.nodes.filter((node) => String(node.id) !== id);
	const connections = workflow.connections.filter(
		(connection) =>
			String(connection.source.nodeId) !== id && String(connection.target.nodeId) !== id
	);
	const layoutNodes = { ...(workflow.layout?.nodes ?? {}) };
	delete layoutNodes[id];
	return assertValid(
		{
			...cloneWorkflow(workflow),
			nodes,
			connections,
			...(workflow.layout ? { layout: { ...workflow.layout, nodes: layoutNodes } } : {})
		},
		registry,
		'Cannot remove workflow node.'
	);
}

export function addConnection(
	workflow: WorkflowDefinition,
	connection: WorkflowConnection,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	const candidate = {
		...cloneWorkflow(workflow),
		connections: [...workflow.connections, connection]
	};
	const connectionValidation = validateConnection(connection, workflow, registry);
	if (!connectionValidation.valid) {
		throw new WorkflowCommandError(
			'Cannot add workflow connection.',
			connectionValidation.issues
		);
	}
	return assertValid(candidate, registry, 'Cannot add workflow connection.');
}

export function removeConnection(
	workflow: WorkflowDefinition,
	connectionId: string,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	if (!workflow.connections.some((connection) => String(connection.id) === connectionId)) {
		throw new WorkflowCommandError(`Workflow connection '${connectionId}' does not exist.`);
	}
	return assertValid(
		{
			...cloneWorkflow(workflow),
			connections: workflow.connections.filter(
				(connection) => String(connection.id) !== connectionId
			)
		},
		registry,
		'Cannot remove workflow connection.'
	);
}

export function updateNodeConfig(
	workflow: WorkflowDefinition,
	nodeId: WorkflowNodeId | string,
	config: JsonObject,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	if (!isJsonObject(config)) {
		throw new WorkflowCommandError('Workflow node config must be a plain JSON object.');
	}
	const id = String(nodeId);
	if (!workflow.nodes.some((node) => String(node.id) === id)) {
		throw new WorkflowCommandError(`Workflow node '${id}' does not exist.`);
	}
	const nodes = workflow.nodes.map((node) =>
		String(node.id) === id ? { ...node, config: { ...node.config, ...config } } : node
	);
	return assertValid(
		{ ...cloneWorkflow(workflow), nodes },
		registry,
		'Cannot update node config.'
	);
}

export function updateNodePosition(
	workflow: WorkflowDefinition,
	nodeId: WorkflowNodeId | string,
	position: WorkflowNodeLayout,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowDefinition {
	const id = String(nodeId);
	if (!workflow.nodes.some((node) => String(node.id) === id)) {
		throw new WorkflowCommandError(`Workflow node '${id}' does not exist.`);
	}
	const layoutNodes = { ...(workflow.layout?.nodes ?? {}), [id]: clonePosition(position) };
	return assertValid(
		{ ...cloneWorkflow(workflow), layout: { ...(workflow.layout ?? {}), nodes: layoutNodes } },
		registry,
		'Cannot update node position.'
	);
}

export function applyWorkflowCommand(
	workflow: WorkflowDefinition,
	command: WorkflowCommand,
	registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY
): WorkflowCommandResult {
	try {
		switch (command.type) {
			case 'add-node':
				return {
					ok: true,
					workflow: addNode(workflow, command.node, command.position, registry)
				};
			case 'remove-node':
				return { ok: true, workflow: removeNode(workflow, command.nodeId, registry) };
			case 'add-connection':
				return {
					ok: true,
					workflow: addConnection(workflow, command.connection, registry)
				};
			case 'remove-connection':
				return {
					ok: true,
					workflow: removeConnection(workflow, command.connectionId, registry)
				};
			case 'update-node-config':
				return {
					ok: true,
					workflow: updateNodeConfig(workflow, command.nodeId, command.config, registry)
				};
			case 'update-node-position':
				return {
					ok: true,
					workflow: updateNodePosition(
						workflow,
						command.nodeId,
						command.position,
						registry
					)
				};
			default: {
				const exhaustive: never = command;
				return exhaustive;
			}
		}
	} catch (cause) {
		if (cause instanceof WorkflowCommandError) return { ok: false, issues: cause.issues };
		return {
			ok: false,
			issues: [
				{
					code: 'invalid_workflow',
					message: cause instanceof Error ? cause.message : String(cause)
				}
			]
		};
	}
}

export class WorkflowHistory {
	private readonly registry: WorkflowNodeRegistry;
	private readonly past: WorkflowDefinition[] = [];
	private readonly future: WorkflowDefinition[] = [];
	private current: WorkflowDefinition;
	private readonly limit: number;

	constructor(
		initial: WorkflowDefinition,
		options: { registry?: WorkflowNodeRegistry; limit?: number } = {}
	) {
		this.registry = options.registry ?? DEFAULT_WORKFLOW_REGISTRY;
		this.current = assertValid(initial, this.registry, 'Cannot create workflow history.');
		this.limit =
			Number.isInteger(options.limit) && (options.limit ?? 0) > 0
				? (options.limit ?? 100)
				: 100;
	}

	get workflow(): WorkflowDefinition {
		return cloneWorkflow(this.current);
	}

	getState(): WorkflowDefinition {
		return this.workflow;
	}

	get canUndo(): boolean {
		return this.past.length > 0;
	}

	get canRedo(): boolean {
		return this.future.length > 0;
	}

	execute(command: WorkflowCommand): WorkflowCommandResult {
		const result = applyWorkflowCommand(this.current, command, this.registry);
		if (!result.ok) return result;
		this.past.push(this.current);
		if (this.past.length > this.limit) this.past.shift();
		this.current = cloneWorkflow(result.workflow);
		this.future.length = 0;
		return { ok: true, workflow: this.workflow };
	}

	apply(command: WorkflowCommand): WorkflowCommandResult {
		return this.execute(command);
	}

	undo(): WorkflowCommandResult {
		const previous = this.past.pop();
		if (!previous) return historyFailure('Nothing to undo.');
		this.future.push(this.current);
		this.current = cloneWorkflow(previous);
		return { ok: true, workflow: this.workflow };
	}

	redo(): WorkflowCommandResult {
		const next = this.future.pop();
		if (!next) return historyFailure('Nothing to redo.');
		this.past.push(this.current);
		this.current = cloneWorkflow(next);
		return { ok: true, workflow: this.workflow };
	}

	clear(): void {
		this.past.length = 0;
		this.future.length = 0;
	}
}

export function createWorkflowHistory(
	initial: WorkflowDefinition,
	options: { registry?: WorkflowNodeRegistry; limit?: number } = {}
): WorkflowHistory {
	return new WorkflowHistory(initial, options);
}

export const workflowCommands = {
	addNode: (node: WorkflowNode, position?: WorkflowNodeLayout): WorkflowCommand => ({
		type: 'add-node',
		node,
		position
	}),
	removeNode: (nodeId: WorkflowNodeId | string): WorkflowCommand => ({
		type: 'remove-node',
		nodeId
	}),
	addConnection: (connection: WorkflowConnection): WorkflowCommand => ({
		type: 'add-connection',
		connection
	}),
	removeConnection: (connectionId: string): WorkflowCommand => ({
		type: 'remove-connection',
		connectionId
	}),
	updateNodeConfig: (nodeId: WorkflowNodeId | string, config: JsonObject): WorkflowCommand => ({
		type: 'update-node-config',
		nodeId,
		config
	}),
	updateNodePosition: (
		nodeId: WorkflowNodeId | string,
		position: WorkflowNodeLayout
	): WorkflowCommand => ({ type: 'update-node-position', nodeId, position })
} as const;

function assertValid(
	workflow: WorkflowDefinition,
	registry: WorkflowNodeRegistry,
	message: string
): WorkflowDefinition {
	const validation = validateWorkflow(workflow, registry);
	if (!validation.valid) throw new WorkflowCommandError(message, validation.issues);
	return cloneWorkflow(workflow);
}

function historyFailure(message: string): WorkflowCommandResult {
	return {
		ok: false,
		issues: [{ code: 'invalid_workflow', message }]
	};
}

function cloneWorkflow(workflow: WorkflowDefinition): WorkflowDefinition {
	return structuredClone(workflow);
}

function cloneNode(node: WorkflowNode): WorkflowNode {
	return structuredClone(node);
}

function clonePosition(position: WorkflowNodeLayout): WorkflowNodeLayout {
	if (!Number.isFinite(position.x) || !Number.isFinite(position.y)) {
		throw new WorkflowCommandError('Workflow node position must contain finite x and y.');
	}
	if (position.width !== undefined && (!Number.isFinite(position.width) || position.width <= 0)) {
		throw new WorkflowCommandError('Workflow node width must be positive when provided.');
	}
	if (
		position.height !== undefined &&
		(!Number.isFinite(position.height) || position.height <= 0)
	) {
		throw new WorkflowCommandError('Workflow node height must be positive when provided.');
	}
	return { ...position };
}

/** Factory for branded ids in command clients that construct new nodes. */
export const createWorkflowNode = (id: string, kind: string, config: JsonObject): WorkflowNode => ({
	id: workflowNodeId(id),
	kind: workflowNodeKind(kind),
	definitionVersion: 1,
	config: cloneJsonObject(config)
});

export const createWorkflowConnection = (
	id: string,
	source: WorkflowConnection['source'],
	target: WorkflowConnection['target']
): WorkflowConnection => ({ id: workflowConnectionId(id), source, target });

function cloneJsonObject(config: JsonObject): JsonObject {
	if (!isJsonObject(config))
		throw new TypeError('Workflow node config must be a plain JSON object.');
	return structuredClone(config);
}
