import {
	WORKFLOW_DATA_TYPES,
	createWorkflowDataType,
	createWorkflowNodeKind,
	createWorkflowPortId,
	isJsonObject,
	workflowDataType,
	workflowPortId,
	type WorkflowDataTypeId,
	type WorkflowDataTypeDescriptor,
	type WorkflowNodeDefinition,
	type WorkflowPortDefinition,
	type WorkflowPortDirection,
	type WorkflowValidationIssue,
	type WorkflowValidationResult
} from './types';

export interface WorkflowPortDefinitionInput extends Omit<
	WorkflowPortDefinition,
	'id' | 'dataType'
> {
	id: string;
	dataType: string;
}

export interface WorkflowNodeDefinitionInput extends Omit<
	WorkflowNodeDefinition,
	'kind' | 'inputs' | 'outputs'
> {
	kind: string;
	inputs: readonly WorkflowPortDefinitionInput[];
	outputs: readonly WorkflowPortDefinitionInput[];
}

export interface WorkflowRegistryOptions {
	/** Register a custom runtime type alongside a node definition. */
	dataTypes?: readonly string[];
}

const DATA_TYPE_DESCRIPTORS: Readonly<Record<string, WorkflowDataTypeDescriptor>> = {
	'automation.event': {
		id: 'automation.event',
		name: 'Automation event',
		description: 'A project event that can start or continue an automation.'
	},
	'automation.result': {
		id: 'automation.result',
		name: 'Automation result',
		description: 'The result emitted by an automation operation.'
	},
	'core.json': {
		id: 'core.json',
		name: 'JSON',
		description: 'Generic structured JSON data.'
	},
	'core.text': {
		id: 'core.text',
		name: 'Text',
		description: 'Plain text data.'
	},
	'core.number': {
		id: 'core.number',
		name: 'Number',
		description: 'A numeric value.'
	},
	'core.boolean': {
		id: 'core.boolean',
		name: 'Boolean',
		description: 'A true or false value.'
	}
};

export class WorkflowNodeRegistry {
	private readonly definitions = new Map<string, Map<number, WorkflowNodeDefinition>>();
	private readonly dataTypes = new Set<string>(WORKFLOW_DATA_TYPES);
	private readonly dataTypeDescriptors = new Map<string, WorkflowDataTypeDescriptor>();

	constructor(definitions: readonly WorkflowNodeDefinitionInput[] = []) {
		for (const definition of definitions) this.register(definition);
	}

	register(
		definition: WorkflowNodeDefinitionInput | WorkflowNodeDefinition,
		options: WorkflowRegistryOptions = {}
	): WorkflowNodeDefinition {
		const normalized = normalizeDefinition(definition);
		const versions = this.definitions.get(normalized.kind);
		if (versions?.has(normalized.version)) {
			throw new Error(
				`Workflow node definition ${normalized.kind}@${normalized.version} is already registered`
			);
		}
		for (const type of options.dataTypes ?? []) {
			if (!createWorkflowDataType(type)) {
				throw new TypeError(`Workflow data type must be a stable identifier: ${type}`);
			}
			this.dataTypes.add(type);
		}
		for (const port of [...normalized.inputs, ...normalized.outputs]) {
			this.dataTypes.add(String(port.dataType));
		}

		if (versions) versions.set(normalized.version, normalized);
		else this.definitions.set(normalized.kind, new Map([[normalized.version, normalized]]));
		return normalized;
	}

	registerDataType(type: string, descriptor?: WorkflowDataTypeDescriptor): WorkflowDataTypeId {
		const parsed = createWorkflowDataType(type);
		if (!parsed) throw new TypeError(`Workflow data type must be a stable identifier: ${type}`);
		this.dataTypes.add(type);
		if (descriptor) this.dataTypeDescriptors.set(type, { ...descriptor, id: parsed });
		return parsed;
	}

	hasDataType(type: string): boolean {
		return this.dataTypes.has(type);
	}

	get(kind: string, version?: number): WorkflowNodeDefinition | undefined {
		const versions = this.definitions.get(kind);
		if (!versions) return undefined;
		if (version !== undefined) return versions.get(version);
		const latest = [...versions.keys()].sort((left, right) => right - left)[0];
		return latest === undefined ? undefined : versions.get(latest);
	}

	has(kind: string, version?: number): boolean {
		return this.get(kind, version) !== undefined;
	}

	list(): readonly WorkflowNodeDefinition[] {
		return [...this.definitions.values()]
			.flatMap((versions) => [...versions.values()])
			.sort((left, right) =>
				left.kind === right.kind
					? left.version - right.version
					: left.kind.localeCompare(right.kind)
			);
	}

	getDataType(type: string): WorkflowDataTypeDescriptor | undefined {
		if (!this.hasDataType(type)) return undefined;
		return (
			this.dataTypeDescriptors.get(type) ??
			DATA_TYPE_DESCRIPTORS[type] ?? {
				id: workflowDataType(type),
				name: type,
				description: `Runtime data type ${type}.`
			}
		);
	}

	listDataTypes(): readonly WorkflowDataTypeDescriptor[] {
		return [...this.dataTypes]
			.sort((left, right) => left.localeCompare(right))
			.map((type) => this.getDataType(type))
			.filter(
				(definition): definition is WorkflowDataTypeDescriptor => definition !== undefined
			);
	}

	clone(): WorkflowNodeRegistry {
		const clone = new WorkflowNodeRegistry();
		for (const type of this.dataTypes) clone.dataTypes.add(type);
		for (const [type, descriptor] of this.dataTypeDescriptors) {
			clone.dataTypeDescriptors.set(type, { ...descriptor });
		}
		for (const definition of this.list()) clone.register(definition);
		return clone;
	}
}

function createDefaultWorkflowRegistry(): WorkflowNodeRegistry {
	return new WorkflowNodeRegistry([
		{
			kind: 'event_trigger',
			version: 1,
			title: 'Trigger',
			description: 'Start a workflow from a project event or manual dispatch.',
			category: 'trigger',
			inputs: [],
			outputs: [port('output', 'output', 'automation.event', 'Event')],
			defaultConfig: () => ({ trigger: 'manual' }),
			validateConfig: validateTriggerConfig
		},
		{
			kind: 'recompute_project_metrics',
			version: 1,
			title: 'Recompute project metrics',
			description: 'Run the currently supported built-in maintenance recipe.',
			category: 'action',
			inputs: [port('input', 'input', 'automation.event', 'Trigger event')],
			outputs: [port('output', 'output', 'automation.result', 'Result')],
			defaultConfig: () => ({ recipe: 'project_maintenance.v1', executable: true }),
			validateConfig: validateProjectMaintenanceConfig
		},
		{
			kind: 'deterministic_action',
			version: 1,
			title: 'Action',
			description: 'Represent a deterministic project operation.',
			category: 'action',
			inputs: [port('input', 'input', 'core.json', 'Input')],
			outputs: [port('output', 'output', 'core.json', 'Output')],
			defaultConfig: () => ({ operation: 'deterministic_action' }),
			validateConfig: validateDeterministicActionConfig
		},
		{
			kind: 'condition',
			version: 1,
			title: 'Condition',
			description: 'Branch a workflow using a deterministic expression.',
			category: 'condition',
			inputs: [port('input', 'input', 'automation.result', 'Input')],
			outputs: [
				port('true', 'output', 'automation.result', 'True'),
				port('false', 'output', 'automation.result', 'False')
			],
			defaultConfig: () => ({ expression: 'confidence >= 0.85' }),
			validateConfig: validateConditionConfig
		},
		{
			kind: 'agent_step',
			version: 1,
			title: 'Agent step',
			description: 'Sketch an analysis step for future recipe support.',
			category: 'agent',
			inputs: [port('input', 'input', 'core.json', 'Input')],
			outputs: [port('output', 'output', 'core.json', 'Output')],
			defaultConfig: () => ({ model: 'review-assistant' }),
			validateConfig: validateAgentConfig
		},
		{
			kind: 'human_gate',
			version: 1,
			title: 'Human gate',
			description: 'Pause at a point that needs a reviewer before continuing.',
			category: 'human_gate',
			inputs: [port('input', 'input', 'automation.result', 'Input')],
			outputs: [port('output', 'output', 'automation.result', 'Approved result')],
			defaultConfig: () => ({ approvals: 1 }),
			validateConfig: validateHumanGateConfig
		}
	]);
}

export const DEFAULT_WORKFLOW_REGISTRY = createDefaultWorkflowRegistry();

export function definitionPorts(
	definition: WorkflowNodeDefinition,
	direction: WorkflowPortDirection
): readonly WorkflowPortDefinition[] {
	return direction === 'input' ? definition.inputs : definition.outputs;
}

function port(
	id: string,
	direction: WorkflowPortDirection,
	dataType: string,
	label: string,
	cardinality?: 'single' | 'many'
): WorkflowPortDefinition {
	const parsedId = workflowPortId(id);
	const parsedType = workflowDataType(dataType);
	return {
		id: parsedId,
		direction,
		dataType: parsedType,
		label,
		cardinality: cardinality ?? (direction === 'output' ? 'many' : 'single')
	};
}

function normalizeDefinition(
	definition: WorkflowNodeDefinitionInput | WorkflowNodeDefinition
): WorkflowNodeDefinition {
	const kind = createWorkflowNodeKind(definition.kind);
	if (!kind) throw new TypeError('Workflow node kind must be a stable identifier');
	if (!Number.isInteger(definition.version) || definition.version < 1) {
		throw new TypeError(
			`Workflow node definition ${kind} must have a positive integer version`
		);
	}
	if (
		typeof definition.title !== 'string' ||
		typeof definition.category !== 'string' ||
		!definition.title.trim() ||
		!definition.category.trim()
	) {
		throw new TypeError(`Workflow node definition ${kind} needs a title and category`);
	}
	if (!Array.isArray(definition.inputs) || !Array.isArray(definition.outputs)) {
		throw new TypeError(
			`Workflow node definition ${kind} must declare input and output arrays`
		);
	}
	if (typeof definition.validateConfig !== 'function') {
		throw new TypeError(`Workflow node definition ${kind} must validate its configuration`);
	}
	const inputIds = new Set<string>();
	for (const candidate of definition.inputs) {
		const normalizedPort = normalizePort(candidate, 'input');
		const key = String(normalizedPort.id);
		if (inputIds.has(key)) throw new TypeError(`Duplicate input port ${kind}.${key}`);
		inputIds.add(key);
	}
	const outputIds = new Set<string>();
	for (const candidate of definition.outputs) {
		const normalizedPort = normalizePort(candidate, 'output');
		const key = String(normalizedPort.id);
		if (outputIds.has(key)) throw new TypeError(`Duplicate output port ${kind}.${key}`);
		outputIds.add(key);
	}
	return {
		kind,
		version: definition.version,
		title: definition.title,
		description: definition.description,
		category: definition.category,
		inputs: definition.inputs.map((value) => normalizePort(value, 'input')),
		outputs: definition.outputs.map((value) => normalizePort(value, 'output')),
		defaultConfig: definition.defaultConfig,
		validateConfig: definition.validateConfig
	};
}

function normalizePort(
	portDefinition: WorkflowPortDefinitionInput | WorkflowPortDefinition,
	direction: WorkflowPortDirection
): WorkflowPortDefinition {
	const id = createWorkflowPortId(portDefinition.id);
	const dataType = createWorkflowDataType(portDefinition.dataType);
	if (
		!id ||
		!dataType ||
		typeof portDefinition.label !== 'string' ||
		!portDefinition.label.trim()
	) {
		throw new TypeError('Workflow ports need stable ids, data types, and labels');
	}
	if (portDefinition.direction !== direction) {
		throw new TypeError(`Workflow ${direction} port ${id} has the wrong direction`);
	}
	if (
		portDefinition.cardinality !== undefined &&
		portDefinition.cardinality !== 'single' &&
		portDefinition.cardinality !== 'many'
	) {
		throw new TypeError(`Workflow port ${id} has an invalid cardinality`);
	}
	return {
		id,
		direction,
		dataType,
		label: portDefinition.label,
		description: portDefinition.description,
		cardinality: portDefinition.cardinality ?? (direction === 'output' ? 'many' : 'single'),
		required: portDefinition.required
	};
}

function validateObjectConfig(config: unknown): WorkflowValidationResult {
	if (isJsonObject(config)) return { valid: true, issues: [] };
	const issue: WorkflowValidationIssue = {
		code: 'invalid_node_config',
		message: 'Node configuration must be a JSON object.'
	};
	return { valid: false, issues: [issue] };
}

function validateTriggerConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const issues: WorkflowValidationIssue[] = [];
	const trigger = resultValue(config, 'trigger');
	const status = resultValue(config, 'status');
	if (trigger !== undefined && !isSupportedTrigger(trigger)) {
		issues.push({
			code: 'invalid_node_config',
			message: 'Trigger config.trigger must be a supported automation trigger.',
			path: 'config.trigger'
		});
	}
	if (status !== undefined && status !== 'active' && status !== 'paused') {
		issues.push({
			code: 'invalid_node_config',
			message: 'Trigger config.status must be active or paused.',
			path: 'config.status'
		});
	}
	return issues.length === 0 ? result : { valid: false, issues };
}

function validateProjectMaintenanceConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const issues: WorkflowValidationIssue[] = [];
	const recipe = resultValue(config, 'recipe');
	const executable = resultValue(config, 'executable');
	if (recipe !== 'project_maintenance.v1') {
		issues.push({
			code: 'invalid_node_config',
			message: 'Project maintenance config.recipe must be project_maintenance.v1.',
			path: 'config.recipe'
		});
	}
	if (typeof executable !== 'boolean') {
		issues.push({
			code: 'invalid_node_config',
			message: 'Project maintenance config.executable must be boolean.',
			path: 'config.executable'
		});
	}
	return issues.length === 0 ? result : { valid: false, issues };
}

function validateDeterministicActionConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const operation = resultValue(config, 'operation');
	if (operation !== undefined && (!isNonEmptyString(operation) || operation.length > 200)) {
		return {
			valid: false,
			issues: [
				{
					code: 'invalid_node_config',
					message: 'Action config.operation must be a string.',
					path: 'config.operation'
				}
			]
		};
	}
	return result;
}

function validateConditionConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const expression = resultValue(config, 'expression');
	if (expression !== undefined && (!isNonEmptyString(expression) || expression.length > 10_000)) {
		return {
			valid: false,
			issues: [
				{
					code: 'invalid_node_config',
					message: 'Condition config.expression must be a string.',
					path: 'config.expression'
				}
			]
		};
	}
	return result;
}

function validateAgentConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const model = resultValue(config, 'model');
	if (model !== undefined && (!isNonEmptyString(model) || model.length > 200)) {
		return {
			valid: false,
			issues: [
				{
					code: 'invalid_node_config',
					message: 'Agent config.model must be a string.',
					path: 'config.model'
				}
			]
		};
	}
	return result;
}

function validateHumanGateConfig(config: unknown): WorkflowValidationResult {
	const result = validateObjectConfig(config);
	if (!result.valid) return result;
	const approvals = resultValue(config, 'approvals');
	if (
		approvals !== undefined &&
		(typeof approvals !== 'number' || !Number.isInteger(approvals) || approvals < 1)
	) {
		return {
			valid: false,
			issues: [
				{
					code: 'invalid_node_config',
					message: 'Human gate config.approvals must be a positive integer.',
					path: 'config.approvals'
				}
			]
		};
	}
	return result;
}

function isNonEmptyString(value: unknown): value is string {
	return typeof value === 'string' && value.trim().length > 0;
}

function resultValue(config: unknown, key: string): unknown {
	return isJsonObject(config) ? config[key] : undefined;
}

function isSupportedTrigger(value: unknown): boolean {
	return (
		value === 'report_added' ||
		value === 'acquisition_completed' ||
		value === 'full_text_attached' ||
		value === 'report_included' ||
		value === 'study_created' ||
		value === 'appraisal_completed' ||
		value === 'manual'
	);
}
