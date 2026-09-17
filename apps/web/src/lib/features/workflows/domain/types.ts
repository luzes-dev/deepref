/**
 * Renderer-neutral workflow vocabulary.
 *
 * This module intentionally contains no editor, DOM, Svelte, or transport
 * dependencies.  A workflow definition is plain data that can be persisted
 * by the browser today and consumed by the Rust executor later.
 */

export const WORKFLOW_SCHEMA_VERSION = 1 as const;
export type WorkflowSchemaVersion = typeof WORKFLOW_SCHEMA_VERSION;

type Brand<Value, Name extends string> = Value & { readonly __brand: Name };

export type WorkflowId = Brand<string, 'WorkflowId'>;
export type WorkflowNodeId = Brand<string, 'WorkflowNodeId'>;
export type WorkflowPortId = Brand<string, 'WorkflowPortId'>;
export type WorkflowConnectionId = Brand<string, 'WorkflowConnectionId'>;
type WorkflowNodeKindValue = Brand<string, 'WorkflowNodeKind'>;
type WorkflowDataTypeValue = Brand<string, 'WorkflowDataType'>;

/** Stable node keys currently exposed by the workflow palette/recipe registry. */
export const WORKFLOW_NODE_KINDS = [
	'event_trigger',
	'recompute_project_metrics',
	'deterministic_action',
	'condition',
	'agent_step',
	'human_gate'
] as const;

export type KnownWorkflowNodeKind = (typeof WORKFLOW_NODE_KINDS)[number];
export type WorkflowNodeKind = KnownWorkflowNodeKind | WorkflowNodeKindValue;

/** Runtime-visible type IDs used by both the browser editor and future Rust runtime. */
export const WORKFLOW_DATA_TYPES = [
	'automation.event',
	'automation.result',
	'core.json',
	'core.text',
	'core.number',
	'core.boolean'
] as const;

export type KnownWorkflowDataType = (typeof WORKFLOW_DATA_TYPES)[number];
export type WorkflowDataTypeId = KnownWorkflowDataType | WorkflowDataTypeValue;

/** Human readable metadata for runtime visible data contracts. */
export interface WorkflowDataTypeDescriptor {
	id: WorkflowDataTypeId;
	name: string;
	description: string;
}

export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
export interface JsonObject {
	readonly [key: string]: JsonValue;
}

export type WorkflowNodeConfig = JsonObject;
export type WorkflowPortDirection = 'input' | 'output';
export type WorkflowPortCardinality = 'single' | 'many';

/** A port is a serialized semantic contract shared by editors and runtimes. */
export interface WorkflowPortDefinition {
	id: WorkflowPortId;
	direction: WorkflowPortDirection;
	dataType: WorkflowDataTypeId;
	label: string;
	description?: string;
	cardinality: WorkflowPortCardinality;
	required?: boolean;
}

export interface WorkflowNodeDefinition {
	kind: WorkflowNodeKind;
	version: number;
	title: string;
	description?: string;
	category: WorkflowNodeCategory;
	inputs: readonly WorkflowPortDefinition[];
	outputs: readonly WorkflowPortDefinition[];
	/** Optional default for palette/fixture construction. It is never persisted. */
	defaultConfig?: () => JsonObject;
	validateConfig: (config: unknown) => WorkflowValidationResult;
}

export type WorkflowNodeCategory =
	| 'source'
	| 'transform'
	| 'screening'
	| 'extraction'
	| 'synthesis'
	| 'human'
	| 'sink'
	| 'trigger'
	| 'action'
	| 'condition'
	| 'agent'
	| 'human_gate'
	| (string & {});

export type WorkflowNode = {
	id: WorkflowNodeId;
	kind: WorkflowNodeKind;
	definitionVersion: number;
	config: WorkflowNodeConfig;
	/**
	 * Optional explicit display override. Renderer adapters resolve display text
	 * as `node.label`, then legacy `metadata.label`, then the definition title.
	 */
	label?: string;
	/** Product metadata is optional and must remain JSON-compatible. */
	metadata?: JsonObject;
};

export interface WorkflowEndpoint {
	nodeId: WorkflowNodeId;
	portId: WorkflowPortId;
}

export interface WorkflowConnection {
	id: WorkflowConnectionId;
	source: WorkflowEndpoint;
	target: WorkflowEndpoint;
	metadata?: JsonObject;
}

export interface WorkflowNodeLayout {
	x: number;
	y: number;
	width?: number;
	height?: number;
}

export type WorkflowNodePosition = WorkflowNodeLayout;

export interface WorkflowLayout {
	nodes: Record<string, WorkflowNodeLayout>;
}

export interface WorkflowDefinition {
	schemaVersion: WorkflowSchemaVersion;
	id: WorkflowId;
	name: string;
	description?: string;
	nodes: WorkflowNode[];
	connections: WorkflowConnection[];
	layout?: WorkflowLayout;
	metadata?: JsonObject;
}

export type WorkflowValidationCode =
	| 'invalid_workflow'
	| 'invalid_schema_version'
	| 'unsupported_schema_version'
	| 'invalid_workflow_id'
	| 'invalid_workflow_name'
	| 'invalid_node'
	| 'invalid_node_id'
	| 'duplicate_node_id'
	| 'unknown_node_kind'
	| 'unsupported_node_definition_version'
	| 'invalid_node_config'
	| 'invalid_port'
	| 'invalid_port_cardinality'
	| 'unknown_port'
	| 'invalid_port_direction'
	| 'unknown_data_type'
	| 'invalid_connection'
	| 'invalid_connection_id'
	| 'duplicate_connection_id'
	| 'dangling_connection'
	| 'incompatible_connection'
	| 'duplicate_connection'
	| 'self_connection'
	| 'input_already_connected'
	| 'invalid_layout'
	| 'missing_layout_node'
	| 'unknown_layout_node'
	| 'graph_cycle'
	| 'legacy_migration_failed';

export interface WorkflowValidationIssue {
	code: WorkflowValidationCode;
	message: string;
	path?: string;
	/** Stable identifiers/details for callers that need to render an error. */
	details?: Readonly<Record<string, string | number>>;
}

export type WorkflowValidationResult =
	| { valid: true; issues: readonly [] }
	| { valid: false; issues: readonly WorkflowValidationIssue[] };

export interface WorkflowCommandSuccess {
	ok: true;
	workflow: WorkflowDefinition;
}

export interface WorkflowCommandFailure {
	ok: false;
	issues: readonly WorkflowValidationIssue[];
}

export type WorkflowCommandResult = WorkflowCommandSuccess | WorkflowCommandFailure;

export function isWorkflowValidationSuccess(
	result: WorkflowValidationResult
): result is Extract<WorkflowValidationResult, { valid: true }> {
	return result.valid;
}

export function createWorkflowId(value: unknown): WorkflowId | null {
	return createIdentifier(value, 'WorkflowId');
}

export function createWorkflowNodeId(value: unknown): WorkflowNodeId | null {
	return createIdentifier(value, 'WorkflowNodeId');
}

export function createWorkflowPortId(value: unknown): WorkflowPortId | null {
	return createIdentifier(value, 'WorkflowPortId');
}

export function createWorkflowConnectionId(value: unknown): WorkflowConnectionId | null {
	return createIdentifier(value, 'WorkflowConnectionId');
}

export function createWorkflowNodeKind(value: unknown): WorkflowNodeKind | null {
	if (!isStableIdentifier(value)) return null;
	return value as WorkflowNodeKindValue;
}

export function createWorkflowDataType(value: unknown): WorkflowDataTypeId | null {
	if (!isStableIdentifier(value)) return null;
	return value as WorkflowDataTypeValue;
}

/** Throwing helpers are intended for trusted construction sites, not JSON parsing. */
export function workflowId(value: string): WorkflowId {
	const parsed = createWorkflowId(value);
	if (!parsed) throw new TypeError('Workflow id must be a non-empty stable identifier');
	return parsed;
}

export function workflowNodeId(value: string): WorkflowNodeId {
	const parsed = createWorkflowNodeId(value);
	if (!parsed) throw new TypeError('Workflow node id must be a non-empty stable identifier');
	return parsed;
}

export function workflowPortId(value: string): WorkflowPortId {
	const parsed = createWorkflowPortId(value);
	if (!parsed) throw new TypeError('Workflow port id must be a non-empty stable identifier');
	return parsed;
}

export function workflowConnectionId(value: string): WorkflowConnectionId {
	const parsed = createWorkflowConnectionId(value);
	if (!parsed)
		throw new TypeError('Workflow connection id must be a non-empty stable identifier');
	return parsed;
}

export function workflowNodeKind(value: string): WorkflowNodeKind {
	const parsed = createWorkflowNodeKind(value);
	if (!parsed) throw new TypeError('Workflow node kind must be a non-empty stable identifier');
	return parsed;
}

export function workflowDataType(value: string): WorkflowDataTypeId {
	const parsed = createWorkflowDataType(value);
	if (!parsed) throw new TypeError('Workflow data type must be a non-empty stable identifier');
	return parsed;
}

export function isJsonObject(value: unknown): value is JsonObject {
	return (
		isJsonValue(value) && typeof value === 'object' && value !== null && !Array.isArray(value)
	);
}

export function isJsonValue(value: unknown): value is JsonValue {
	return isJsonValueAt(value, new Set<object>());
}

/**
 * JSON.parse only creates plain objects and dense arrays.  Runtime callers can
 * hand us Date, Map, class instances, sparse arrays, getters, or cycles, so
 * those values must be rejected before they reach persistence or a node
 * validator. Non-enumerable properties are rejected because object
 * serialization would silently omit them. `ancestors` tracks the current
 * recursion path and therefore still permits the same plain object to be
 * referenced by two JSON fields.
 */
function isJsonValueAt(value: unknown, ancestors: Set<object>): value is JsonValue {
	if (value === null || typeof value === 'string' || typeof value === 'boolean') return true;
	if (typeof value === 'number') return Number.isFinite(value);
	if (typeof value !== 'object') return false;

	const objectValue = value as object;
	if (ancestors.has(objectValue)) return false;
	ancestors.add(objectValue);
	try {
		if (Array.isArray(value)) {
			if (Object.getPrototypeOf(value) !== Array.prototype) return false;
			if (Object.getOwnPropertySymbols(value).length > 0) return false;
			const propertyNames = Object.getOwnPropertyNames(value);
			if (propertyNames.length !== value.length + 1) return false;
			for (const key of propertyNames) {
				if (key === 'length') continue;
				const index = Number(key);
				if (
					!Number.isInteger(index) ||
					index < 0 ||
					index >= value.length ||
					String(index) !== key
				) {
					return false;
				}
				const descriptor = Object.getOwnPropertyDescriptor(value, key);
				if (!descriptor || descriptor.enumerable !== true || !('value' in descriptor)) {
					return false;
				}
				if (!isJsonValueAt(descriptor.value, ancestors)) return false;
			}
			return true;
		}

		const prototype = Object.getPrototypeOf(value);
		if (prototype !== Object.prototype && prototype !== null) return false;
		if (Object.getOwnPropertySymbols(value).length > 0) return false;
		for (const key of Object.getOwnPropertyNames(value)) {
			const descriptor = Object.getOwnPropertyDescriptor(value, key);
			if (!descriptor || descriptor.enumerable !== true || !('value' in descriptor)) {
				return false;
			}
			if (!isJsonValueAt(descriptor.value, ancestors)) return false;
		}
		return true;
	} catch {
		return false;
	} finally {
		ancestors.delete(objectValue);
	}
}

function createIdentifier<Name extends string>(
	value: unknown,
	brand: Name
): Brand<string, Name> | null {
	void brand;
	if (!isStableIdentifier(value)) return null;
	return value as Brand<string, Name>;
}

function isStableIdentifier(value: unknown): value is string {
	return (
		typeof value === 'string' &&
		value.length > 0 &&
		value.trim() === value &&
		value.length <= 200 &&
		/^[A-Za-z0-9][A-Za-z0-9._:-]*$/.test(value)
	);
}
