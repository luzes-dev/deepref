import { DEFAULT_WORKFLOW_REGISTRY, type WorkflowNodeRegistry } from './registry';
import { migrateLegacyAutomationGraph, type LegacyMigrationOptions } from './migration';
import { validateWorkflow } from './validation';
import {
	WORKFLOW_SCHEMA_VERSION,
	createWorkflowConnectionId,
	createWorkflowId,
	createWorkflowNodeId,
	createWorkflowNodeKind,
	createWorkflowPortId,
	isJsonObject,
	type JsonObject,
	type JsonValue,
	type WorkflowConnection,
	type WorkflowDefinition,
	type WorkflowLayout,
	type WorkflowNode,
	type WorkflowNodeLayout,
	type WorkflowEndpoint,
	type WorkflowValidationIssue
} from './types';

export const CURRENT_SCHEMA_VERSION = WORKFLOW_SCHEMA_VERSION;

export interface WorkflowSerializationOptions {
	registry?: WorkflowNodeRegistry;
	/** Parse the version-one XYFlow browser draft when the canonical shape is absent. */
	allowLegacyAutomationGraph?: boolean;
	legacy?: LegacyMigrationOptions;
}

export interface SerializationSuccess<T> {
	success: true;
	data: T;
	migrated?: boolean;
	warnings?: readonly string[];
}

export interface SerializationFailure {
	success: false;
	error: string;
	issues: readonly WorkflowValidationIssue[];
	/** The exact input is retained so callers can offer recovery/export. */
	raw?: string;
}

export type SerializationResult<T> = SerializationSuccess<T> | SerializationFailure;

export class WorkflowSerializationError extends Error {
	readonly issues: readonly WorkflowValidationIssue[];
	readonly raw: unknown;

	constructor(message: string, issues: readonly WorkflowValidationIssue[] = [], raw?: unknown) {
		super(message);
		this.name = 'WorkflowSerializationError';
		this.issues = issues;
		this.raw = raw;
	}
}

/**
 * Serialize the validated domain document without renderer or executable
 * objects. Property order is explicit to keep local drafts diffable.
 */
export function serializeWorkflowDefinition(
	workflow: WorkflowDefinition,
	prettyOrOptions: boolean | WorkflowSerializationOptions = false,
	maybeRegistry?: WorkflowNodeRegistry
): string {
	const options = normalizeOptions(prettyOrOptions, maybeRegistry);
	const validation = validateWorkflow(workflow, options.registry);
	if (!validation.valid) {
		throw new WorkflowSerializationError(
			'Cannot serialize an invalid workflow definition.',
			validation.issues,
			workflow
		);
	}
	const canonical = canonicalWorkflow(workflow);
	try {
		const serialized = JSON.stringify(canonical, null, options.pretty ? 2 : undefined);
		if (serialized === undefined) {
			throw new TypeError('JSON.stringify returned no document');
		}
		return serialized;
	} catch (cause) {
		const issue: WorkflowValidationIssue = {
			code: 'invalid_workflow',
			message: `Workflow could not be serialized: ${errorMessage(cause)}`
		};
		throw new WorkflowSerializationError(issue.message, [issue], workflow);
	}
}

/** Parse a JSON document and return structured issues while retaining raw input. */
export function deserializeWorkflowDefinition(
	jsonString: string,
	options: WorkflowSerializationOptions = {}
): SerializationResult<WorkflowDefinition> {
	if (typeof jsonString !== 'string' || jsonString.trim().length === 0) {
		return failure(
			'Input must be a non-empty workflow JSON string.',
			[
				{
					code: 'invalid_workflow',
					message: 'Input must be a non-empty workflow JSON string.'
				}
			],
			jsonString
		);
	}

	let parsed: unknown;
	try {
		parsed = JSON.parse(jsonString);
	} catch (cause) {
		return failure(
			`Invalid JSON: ${errorMessage(cause)}`,
			[
				{
					code: 'invalid_workflow',
					message: `Invalid JSON: ${errorMessage(cause)}`
				}
			],
			jsonString
		);
	}

	if (!isJsonObject(parsed)) {
		return failure(
			'Serialized workflow must be a plain JSON object.',
			[
				{
					code: 'invalid_workflow',
					message: 'Serialized workflow must be a plain JSON object.'
				}
			],
			jsonString
		);
	}

	if (isLegacyEnvelope(parsed)) {
		if (options.allowLegacyAutomationGraph === false) {
			return failure(
				'Serialized value is a legacy automation graph; migrate it explicitly before loading.',
				[
					{
						code: 'legacy_migration_failed',
						message:
							'Serialized value is a legacy automation graph; migrate it explicitly before loading.'
					}
				],
				jsonString
			);
		}
		const migrated = migrateLegacyAutomationGraph(parsed, {
			...options.legacy,
			...((options.legacy?.registry ?? options.registry)
				? { registry: options.legacy?.registry ?? options.registry }
				: {})
		});
		if (!migrated.ok) {
			return failure(
				'Legacy automation graph migration failed.',
				migrated.issues,
				jsonString
			);
		}
		return {
			success: true,
			data: migrated.workflow,
			migrated: true,
			warnings: migrated.warnings
		};
	}

	const registry = options.registry ?? DEFAULT_WORKFLOW_REGISTRY;
	if (
		parsed.schemaVersion === undefined &&
		typeof parsed.version === 'number' &&
		parsed.version > WORKFLOW_SCHEMA_VERSION
	) {
		return failure(
			`Workflow schema version ${parsed.version} is newer than the supported version ${WORKFLOW_SCHEMA_VERSION}.`,
			[
				{
					code: 'unsupported_schema_version',
					message: `Workflow schema version ${parsed.version} is newer than the supported version ${WORKFLOW_SCHEMA_VERSION}.`,
					path: 'version'
				}
			],
			jsonString
		);
	}
	const validation = validateWorkflow(parsed, registry);
	if (!validation.valid) {
		const error = validation.issues.map((issue) => issue.message).join(' ');
		return failure(error || 'Workflow definition is invalid.', validation.issues, jsonString);
	}
	const workflow = narrowWorkflowDefinition(parsed);
	if (!workflow) {
		return failure(
			'Workflow definition could not be narrowed after validation.',
			[
				{
					code: 'invalid_workflow',
					message: 'Workflow definition could not be narrowed after validation.'
				}
			],
			jsonString
		);
	}

	return {
		success: true,
		data: workflow
	};
}

function canonicalWorkflow(workflow: WorkflowDefinition): JsonObject {
	const canonical: JsonObject = {
		schemaVersion: workflow.schemaVersion,
		id: workflow.id,
		name: workflow.name,
		nodes: workflow.nodes.map((node) => ({
			id: node.id,
			kind: node.kind,
			definitionVersion: node.definitionVersion,
			...(node.label !== undefined ? { label: node.label } : {}),
			config: node.config,
			...(node.metadata !== undefined ? { metadata: node.metadata } : {})
		})),
		connections: workflow.connections.map((connection) => ({
			id: connection.id,
			source: {
				nodeId: connection.source.nodeId,
				portId: connection.source.portId
			},
			target: {
				nodeId: connection.target.nodeId,
				portId: connection.target.portId
			},
			...(connection.metadata !== undefined ? { metadata: connection.metadata } : {})
		})),
		...(workflow.description !== undefined ? { description: workflow.description } : {}),
		...(workflow.layout !== undefined ? { layout: canonicalLayout(workflow.layout) } : {}),
		...(workflow.metadata !== undefined ? { metadata: workflow.metadata } : {})
	};
	return canonical;
}

function canonicalLayout(layout: WorkflowLayout): JsonObject {
	const nodes: Record<string, JsonValue> = {};
	for (const [id, position] of Object.entries(layout.nodes)) {
		nodes[id] = canonicalNodeLayout(position);
	}
	return { nodes };
}

function canonicalNodeLayout(position: WorkflowNodeLayout): JsonObject {
	return {
		x: position.x,
		y: position.y,
		...(position.width !== undefined ? { width: position.width } : {}),
		...(position.height !== undefined ? { height: position.height } : {})
	};
}

/**
 * Build trusted domain objects after validation has completed. Keeping this
 * conversion separate from the validator avoids running the boundary checks a
 * second time merely to obtain a type predicate.
 */
function narrowWorkflowDefinition(value: JsonObject): WorkflowDefinition | null {
	if (value.schemaVersion !== WORKFLOW_SCHEMA_VERSION) return null;
	const id = createWorkflowId(value.id);
	if (
		!id ||
		typeof value.name !== 'string' ||
		!Array.isArray(value.nodes) ||
		!Array.isArray(value.connections)
	) {
		return null;
	}
	const nodes: WorkflowNode[] = [];
	for (const valueNode of value.nodes) {
		const node = narrowWorkflowNode(valueNode);
		if (!node) return null;
		nodes.push(node);
	}
	const connections: WorkflowConnection[] = [];
	for (const valueConnection of value.connections) {
		const connection = narrowWorkflowConnection(valueConnection);
		if (!connection) return null;
		connections.push(connection);
	}
	const layout = value.layout === undefined ? undefined : narrowWorkflowLayout(value.layout);
	if (value.layout !== undefined && !layout) return null;
	return {
		schemaVersion: WORKFLOW_SCHEMA_VERSION,
		id,
		name: value.name,
		...(typeof value.description === 'string' ? { description: value.description } : {}),
		nodes,
		connections,
		...(layout ? { layout } : {}),
		...(isJsonObject(value.metadata) ? { metadata: value.metadata } : {})
	};
}

function narrowWorkflowNode(value: JsonValue): WorkflowNode | null {
	if (!isJsonObject(value)) return null;
	const id = createWorkflowNodeId(value.id);
	const kind = createWorkflowNodeKind(value.kind);
	const definitionVersion = positiveInteger(value.definitionVersion);
	if (!id || !kind || definitionVersion === null || !isJsonObject(value.config)) return null;
	return {
		id,
		kind,
		definitionVersion,
		config: value.config,
		...(typeof value.label === 'string' ? { label: value.label } : {}),
		...(isJsonObject(value.metadata) ? { metadata: value.metadata } : {})
	};
}

function narrowWorkflowConnection(value: JsonValue): WorkflowConnection | null {
	if (!isJsonObject(value)) return null;
	const id = createWorkflowConnectionId(value.id);
	const source = narrowWorkflowEndpoint(value.source);
	const target = narrowWorkflowEndpoint(value.target);
	if (!id || !source || !target) return null;
	return {
		id,
		source,
		target,
		...(isJsonObject(value.metadata) ? { metadata: value.metadata } : {})
	};
}

function narrowWorkflowEndpoint(value: JsonValue | undefined): WorkflowEndpoint | null {
	if (!isJsonObject(value)) return null;
	const nodeId = createWorkflowNodeId(value.nodeId);
	const portId = createWorkflowPortId(value.portId);
	return nodeId && portId ? { nodeId, portId } : null;
}

function narrowWorkflowLayout(value: JsonValue): WorkflowLayout | null {
	if (!isJsonObject(value) || !isJsonObject(value.nodes)) return null;
	const nodes: Record<string, WorkflowNodeLayout> = {};
	for (const [id, valuePosition] of Object.entries(value.nodes)) {
		if (
			!isJsonObject(valuePosition) ||
			!isFiniteNumber(valuePosition.x) ||
			!isFiniteNumber(valuePosition.y)
		) {
			return null;
		}
		const width = optionalPositiveNumber(valuePosition.width);
		const height = optionalPositiveNumber(valuePosition.height);
		if (valuePosition.width !== undefined && width === null) return null;
		if (valuePosition.height !== undefined && height === null) return null;
		nodes[id] = {
			x: valuePosition.x,
			y: valuePosition.y,
			...(width !== null ? { width } : {}),
			...(height !== null ? { height } : {})
		};
	}
	return { nodes };
}

function positiveInteger(value: JsonValue | undefined): number | null {
	return typeof value === 'number' && Number.isInteger(value) && value >= 1 ? value : null;
}

function optionalPositiveNumber(value: JsonValue | undefined): number | null {
	return typeof value === 'number' && Number.isFinite(value) && value > 0 ? value : null;
}

function isFiniteNumber(value: JsonValue | undefined): value is number {
	return typeof value === 'number' && Number.isFinite(value);
}

function normalizeOptions(
	prettyOrOptions: boolean | WorkflowSerializationOptions,
	maybeRegistry: WorkflowNodeRegistry | undefined
): WorkflowSerializationOptions & { pretty: boolean } {
	if (typeof prettyOrOptions === 'boolean') {
		return { pretty: prettyOrOptions, ...(maybeRegistry ? { registry: maybeRegistry } : {}) };
	}
	return { ...prettyOrOptions, pretty: false };
}

function isLegacyEnvelope(value: JsonObject): boolean {
	return (
		value.schemaVersion === undefined &&
		value.version === 1 &&
		Array.isArray(value.nodes) &&
		Array.isArray(value.edges)
	);
}

function failure(
	error: string,
	issues: readonly WorkflowValidationIssue[],
	raw: string
): SerializationFailure {
	return { success: false, error, issues, raw };
}

function errorMessage(cause: unknown): string {
	return cause instanceof Error ? cause.message : String(cause);
}
