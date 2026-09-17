import { describe, expect, it } from 'vitest';

import {
	addConnection,
	addNode,
	applyWorkflowCommand,
	createEmptyWorkflow,
	createWorkflowHistory,
	removeNode,
	updateNodeConfig,
	updateNodePosition,
	workflowCommands
} from './commands';
import { isLegacyAutomationGraph, migrateLegacyAutomationGraph } from './migration';
import {
	DEFAULT_WORKFLOW_REGISTRY,
	WorkflowNodeRegistry,
	type WorkflowNodeDefinitionInput,
	type WorkflowPortDefinitionInput
} from './registry';
import {
	deserializeWorkflowDefinition,
	serializeWorkflowDefinition,
	WorkflowSerializationError
} from './serialization';
import {
	createWorkflowConnectionId,
	createWorkflowId,
	createWorkflowNodeId,
	createWorkflowNodeKind,
	createWorkflowPortId,
	isJsonObject,
	isJsonValue,
	workflowConnectionId,
	workflowDataType,
	workflowId,
	workflowNodeId,
	workflowNodeKind,
	workflowPortId,
	type JsonObject,
	type WorkflowConnection,
	type WorkflowDefinition,
	type WorkflowNode,
	type WorkflowValidationResult
} from './types';
import {
	detectCycles,
	validateConnection,
	validatePortCompatibility,
	validateWorkflow
} from './validation';

const FIXTURE_COLLECTION_TYPE = 'fixture.paper.collection';
const FIXTURE_TEXT_TYPE = 'core.text';

function validConfig(value: unknown): WorkflowValidationResult {
	return isJsonObject(value)
		? { valid: true, issues: [] }
		: {
				valid: false,
				issues: [
					{
						code: 'invalid_node_config',
						message: 'Fixture configuration must be a JSON object.'
					}
				]
			};
}

function fixturePort(
	id: string,
	direction: 'input' | 'output',
	dataType = FIXTURE_COLLECTION_TYPE
): WorkflowPortDefinitionInput {
	return {
		id,
		direction,
		dataType,
		label: id,
		cardinality: direction === 'input' ? 'single' : 'many'
	};
}

function fixtureDefinition(
	kind: string,
	inputs: readonly WorkflowPortDefinitionInput[],
	outputs: readonly WorkflowPortDefinitionInput[],
	version = 1
): WorkflowNodeDefinitionInput {
	return {
		kind,
		version,
		title: kind,
		category: 'fixture',
		inputs,
		outputs,
		validateConfig: validConfig
	};
}

function fixtureRegistry(): WorkflowNodeRegistry {
	const registry = new WorkflowNodeRegistry();
	registry.register(fixtureDefinition('fixture.source', [], [fixturePort('output', 'output')]));
	registry.register(
		fixtureDefinition(
			'fixture.transform',
			[fixturePort('input', 'input')],
			[fixturePort('output', 'output')]
		)
	);
	registry.register(
		fixtureDefinition(
			'fixture.branch',
			[fixturePort('input', 'input')],
			[fixturePort('left', 'output'), fixturePort('right', 'output')]
		)
	);
	registry.register(
		fixtureDefinition(
			'fixture.merge',
			[fixturePort('left', 'input'), fixturePort('right', 'input')],
			[fixturePort('output', 'output')]
		)
	);
	registry.register(
		fixtureDefinition('fixture.review', [fixturePort('input', 'input', FIXTURE_TEXT_TYPE)], [])
	);
	return registry;
}

function fixtureNode(id: string, kind: string, config: JsonObject = {}): WorkflowNode {
	return {
		id: workflowNodeId(id),
		kind: workflowNodeKind(kind),
		definitionVersion: 1,
		config
	};
}

function fixtureConnection(
	id: string,
	sourceNodeId: string,
	sourcePortId: string,
	targetNodeId: string,
	targetPortId: string
): WorkflowConnection {
	return {
		id: workflowConnectionId(id),
		source: {
			nodeId: workflowNodeId(sourceNodeId),
			portId: workflowPortId(sourcePortId)
		},
		target: {
			nodeId: workflowNodeId(targetNodeId),
			portId: workflowPortId(targetPortId)
		}
	};
}

function workflow(
	nodes: readonly WorkflowNode[],
	connections: readonly WorkflowConnection[] = []
): WorkflowDefinition {
	return {
		schemaVersion: 1,
		id: workflowId('fixture-workflow'),
		name: 'Fixture workflow',
		nodes: [...nodes],
		connections: [...connections]
	};
}

function linearFixture(): WorkflowDefinition {
	const source = fixtureNode('source', 'fixture.source', { query: 'trial' });
	const transform = fixtureNode('transform', 'fixture.transform', { deduplicate: true });
	const review = fixtureNode('review', 'fixture.transform', { stage: 'review' });
	return workflow(
		[source, transform, review],
		[
			fixtureConnection('source-transform', 'source', 'output', 'transform', 'input'),
			fixtureConnection('transform-review', 'transform', 'output', 'review', 'input')
		]
	);
}

function branchedFixture(): WorkflowDefinition {
	const source = fixtureNode('source', 'fixture.source');
	const branch = fixtureNode('branch', 'fixture.branch');
	const merge = fixtureNode('merge', 'fixture.merge');
	return workflow(
		[source, branch, merge],
		[
			fixtureConnection('source-branch', 'source', 'output', 'branch', 'input'),
			fixtureConnection('branch-left', 'branch', 'left', 'merge', 'left'),
			fixtureConnection('branch-right', 'branch', 'right', 'merge', 'right')
		]
	);
}

function invalidTypedFixture(): WorkflowDefinition {
	const source = fixtureNode('source', 'fixture.source');
	const review = fixtureNode('review', 'fixture.review');
	return workflow(
		[source, review],
		[fixtureConnection('wrong-type', 'source', 'output', 'review', 'input')]
	);
}

function issueCodes(result: WorkflowValidationResult): string[] {
	return result.valid ? [] : result.issues.map((issue) => issue.code);
}

describe('workflow domain fixtures', () => {
	it('validates linear and branched typed workflows without a renderer', () => {
		const registry = fixtureRegistry();

		expect(validateWorkflow(linearFixture(), registry)).toEqual({
			valid: true,
			issues: []
		});
		expect(validateWorkflow(branchedFixture(), registry)).toEqual({
			valid: true,
			issues: []
		});
	});

	it('rejects a connection whose serialized port types are incompatible', () => {
		const result = validateWorkflow(invalidTypedFixture(), fixtureRegistry());

		expect(result.valid).toBe(false);
		expect(issueCodes(result)).toContain('incompatible_connection');
	});

	it('detects cycles as an execution concern without making the IR DAG-only', () => {
		const fixture = linearFixture();
		const cycle = fixtureConnection('review-source', 'review', 'output', 'source', 'input');
		const result = detectCycles(fixture.nodes, [...fixture.connections, cycle]);

		expect(result.hasCycles).toBe(true);
		expect(result.cycles[0]?.map(String)).toEqual(['source', 'transform', 'review', 'source']);
	});
});

describe('workflow JSON boundary', () => {
	it('accepts JSON values while rejecting non-JSON, sparse, and cyclic values', () => {
		const shared = { value: 'ok' };
		const repeated = { left: shared, right: shared };
		const cyclic: Record<string, unknown> = {};
		cyclic.self = cyclic;
		const sparse: unknown[] = [];
		sparse.length = 1;

		expect(isJsonObject(repeated)).toBe(true);
		expect(isJsonValue(repeated)).toBe(true);
		expect(isJsonValue(new Date())).toBe(false);
		expect(isJsonValue(Number.NaN)).toBe(false);
		expect(isJsonValue(cyclic)).toBe(false);
		expect(isJsonValue(sparse)).toBe(false);
	});

	it('rejects accessors, symbols, custom arrays, and non-enumerable properties', () => {
		const accessor: Record<string, unknown> = {};
		Object.defineProperty(accessor, 'value', {
			enumerable: true,
			get: () => 'not plain JSON'
		});
		const withSymbol: Record<string, unknown> = { value: 'ok' };
		Object.defineProperty(withSymbol, Symbol('extra'), { enumerable: true, value: 'hidden' });
		const withHiddenProperty: Record<string, unknown> = {};
		Object.defineProperty(withHiddenProperty, 'secret', {
			enumerable: false,
			value: 'dropped by JSON.stringify'
		});
		const hiddenArray = [1];
		Object.defineProperty(hiddenArray, '0', {
			configurable: true,
			enumerable: false,
			value: 1,
			writable: true
		});
		class CustomArray extends Array<number> {}

		expect(isJsonValue(accessor)).toBe(false);
		expect(isJsonValue(withSymbol)).toBe(false);
		expect(isJsonValue(withHiddenProperty)).toBe(false);
		expect(isJsonObject(withHiddenProperty)).toBe(false);
		expect(JSON.stringify(withHiddenProperty)).toBe('{}');
		expect(isJsonValue(hiddenArray)).toBe(false);
		expect(isJsonValue(new CustomArray(1, 2))).toBe(false);
	});

	it('serializes canonical fields and round-trips the renderer-neutral document', () => {
		const source = fixtureNode('source', 'fixture.source', { query: 'trial' });
		const transform = fixtureNode('transform', 'fixture.transform', { deduplicate: true });
		const document: WorkflowDefinition = {
			...workflow(
				[source, transform],
				[fixtureConnection('source-transform', 'source', 'output', 'transform', 'input')]
			),
			description: 'A persisted fixture',
			layout: {
				nodes: {
					source: { x: 40, y: 80, width: 240 },
					transform: { x: 380, y: 80, height: 176 }
				}
			},
			metadata: { owner: 'test', tags: ['typed', 'fixture'] }
		};
		const registry = fixtureRegistry();
		const json = serializeWorkflowDefinition(document, { registry });
		const parsedJson: unknown = JSON.parse(json);

		expect(parsedJson).toEqual({
			schemaVersion: 1,
			id: 'fixture-workflow',
			name: 'Fixture workflow',
			nodes: [
				{
					id: 'source',
					kind: 'fixture.source',
					definitionVersion: 1,
					config: { query: 'trial' }
				},
				{
					id: 'transform',
					kind: 'fixture.transform',
					definitionVersion: 1,
					config: { deduplicate: true }
				}
			],
			connections: [
				{
					id: 'source-transform',
					source: { nodeId: 'source', portId: 'output' },
					target: { nodeId: 'transform', portId: 'input' }
				}
			],
			description: 'A persisted fixture',
			layout: {
				nodes: {
					source: { x: 40, y: 80, width: 240 },
					transform: { x: 380, y: 80, height: 176 }
				}
			},
			metadata: { owner: 'test', tags: ['typed', 'fixture'] }
		});

		const result = deserializeWorkflowDefinition(json, { registry });
		expect(result.success).toBe(true);
		if (result.success) expect(result.data).toEqual(document);
	});

	it('preserves explicit labels alongside legacy metadata labels', () => {
		const document = workflow([
			{
				...fixtureNode('source', 'fixture.source'),
				label: 'Explicit title',
				metadata: { label: 'Legacy title' }
			}
		]);
		const registry = fixtureRegistry();
		const result = deserializeWorkflowDefinition(
			serializeWorkflowDefinition(document, { registry }),
			{ registry }
		);

		expect(result.success).toBe(true);
		if (result.success) {
			expect(result.data.nodes[0]?.label).toBe('Explicit title');
			expect(result.data.nodes[0]?.metadata?.label).toBe('Legacy title');
		}
	});

	it('rejects future or malformed schema documents and retains the raw input', () => {
		const registry = fixtureRegistry();
		const future = JSON.stringify({ ...linearFixture(), schemaVersion: 2 });
		const malformed = '{"schemaVersion":1';

		const futureResult = deserializeWorkflowDefinition(future, { registry });
		const malformedResult = deserializeWorkflowDefinition(malformed, { registry });

		expect(futureResult.success).toBe(false);
		if (!futureResult.success) {
			expect(futureResult.raw).toBe(future);
			expect(issueCodes({ valid: false, issues: futureResult.issues })).toContain(
				'unsupported_schema_version'
			);
		}
		expect(malformedResult.success).toBe(false);
		if (!malformedResult.success) expect(malformedResult.raw).toBe(malformed);
	});

	it('refuses to serialize an invalid document', () => {
		const document = invalidTypedFixture();

		expect(() =>
			serializeWorkflowDefinition(document, { registry: fixtureRegistry() })
		).toThrow(WorkflowSerializationError);
	});
});

describe('workflow shape validation', () => {
	it('rejects duplicate node and connection ids', () => {
		const registry = fixtureRegistry();
		const source = fixtureNode('source', 'fixture.source');
		const transform = fixtureNode('transform', 'fixture.transform');
		const connection = fixtureConnection(
			'source-transform',
			'source',
			'output',
			'transform',
			'input'
		);

		const duplicateNodes = validateWorkflow(workflow([source, { ...source }]), registry);
		const duplicateConnections = validateWorkflow(
			workflow([source, transform], [connection, { ...connection }]),
			registry
		);
		const invalidMetadataConnection = { ...connection, metadata: 'not an object' };

		expect(issueCodes(duplicateNodes)).toContain('duplicate_node_id');
		expect(issueCodes(duplicateConnections)).toContain('duplicate_connection_id');
		expect(
			issueCodes(
				validateConnection(
					invalidMetadataConnection,
					workflow([source, transform]),
					registry
				)
			)
		).toContain('invalid_connection');
	});

	it('rejects invalid workflow, node, and connection ids at the JSON boundary', () => {
		const registry = fixtureRegistry();
		const source = {
			id: 'source',
			kind: 'fixture.source',
			definitionVersion: 1,
			config: {}
		};
		const base = {
			schemaVersion: 1,
			id: 'fixture-workflow',
			name: 'Fixture workflow',
			nodes: [source],
			connections: []
		};
		const invalidWorkflowId = { ...base, id: 'workflow/id' };
		const invalidNodeId = { ...base, nodes: [{ ...source, id: ' source' }] };
		const invalidConnectionId = {
			...base,
			nodes: [
				source,
				{ id: 'target', kind: 'fixture.transform', definitionVersion: 1, config: {} }
			],
			connections: [
				{
					id: 'connection/id',
					source: { nodeId: 'source', portId: 'output' },
					target: { nodeId: 'target', portId: 'input' }
				}
			]
		};

		expect(issueCodes(validateWorkflow(invalidWorkflowId, registry))).toContain(
			'invalid_workflow_id'
		);
		expect(issueCodes(validateWorkflow(invalidNodeId, registry))).toContain('invalid_node_id');
		expect(issueCodes(validateWorkflow(invalidConnectionId, registry))).toContain(
			'invalid_connection_id'
		);
	});

	it('rejects dangling connections and unknown node kinds', () => {
		const registry = fixtureRegistry();
		const source = fixtureNode('source', 'fixture.source');
		const dangling = fixtureConnection('dangling', 'source', 'output', 'missing', 'input');
		const unknown = fixtureNode('mystery', 'fixture.missing');

		expect(issueCodes(validateWorkflow(workflow([source], [dangling]), registry))).toContain(
			'dangling_connection'
		);
		expect(issueCodes(validateWorkflow(workflow([unknown]), registry))).toContain(
			'unknown_node_kind'
		);
	});

	it('rejects unsupported definition versions and unknown ports', () => {
		const registry = fixtureRegistry();
		const source = fixtureNode('source', 'fixture.source');
		const transform = fixtureNode('transform', 'fixture.transform');
		const unsupportedVersion = {
			...source,
			definitionVersion: 2
		};
		const unknownPort = fixtureConnection(
			'unknown-port',
			'source',
			'missing',
			'transform',
			'input'
		);

		expect(issueCodes(validateWorkflow(workflow([unsupportedVersion]), registry))).toContain(
			'unsupported_node_definition_version'
		);
		expect(
			issueCodes(validateWorkflow(workflow([source, transform], [unknownPort]), registry))
		).toContain('unknown_port');
	});

	it('rejects bad node configuration through the registered validator', () => {
		const trigger: WorkflowNode = {
			id: workflowNodeId('trigger'),
			kind: workflowNodeKind('event_trigger'),
			definitionVersion: 1,
			config: { trigger: 'unsupported-trigger' }
		};

		expect(
			issueCodes(validateWorkflow(workflow([trigger]), DEFAULT_WORKFLOW_REGISTRY))
		).toContain('invalid_node_config');
	});

	it('rejects a second connection to a single-cardinality input', () => {
		const registry = fixtureRegistry();
		const source = fixtureNode('source', 'fixture.source');
		const secondSource = fixtureNode('source-2', 'fixture.source');
		const transform = fixtureNode('transform', 'fixture.transform');
		const first = fixtureConnection('first', 'source', 'output', 'transform', 'input');
		const second = fixtureConnection('second', 'source-2', 'output', 'transform', 'input');

		const result = validateWorkflow(
			workflow([source, secondSource, transform], [first, second]),
			registry
		);
		expect(issueCodes(result)).toContain('input_already_connected');
		expect(
			validateConnection(
				second,
				workflow([source, secondSource, transform], [first]),
				registry
			)
		).toEqual({
			valid: false,
			issues: [expect.objectContaining({ code: 'input_already_connected' })]
		});
	});
});

describe('workflow node registry', () => {
	it('keeps definition versions separate and exposes the latest version', () => {
		const registry = fixtureRegistry();
		const versionTwo = fixtureDefinition(
			'fixture.source',
			[],
			[fixturePort('output', 'output')],
			2
		);

		registry.register(versionTwo);

		expect(registry.get('fixture.source', 1)?.version).toBe(1);
		expect(registry.get('fixture.source')?.version).toBe(2);
		expect(registry.has('fixture.source', 2)).toBe(true);
		expect(registry.get('fixture.missing')).toBeUndefined();
		expect(() => registry.register(versionTwo)).toThrow('already registered');
	});

	it('uses registered runtime data types for custom port compatibility', () => {
		const registry = fixtureRegistry();
		const source = registry.get('fixture.source', 1);
		const transform = registry.get('fixture.transform', 1);

		expect(source).toBeDefined();
		expect(transform).toBeDefined();
		if (!source || !transform) return;
		expect(
			validatePortCompatibility(source.outputs[0], transform.inputs[0], { registry })
		).toEqual({
			valid: true,
			issues: []
		});
		expect(
			validatePortCompatibility(
				{ ...source.outputs[0], dataType: workflowDataType('fixture.unknown') },
				transform.inputs[0],
				{ registry }
			).valid
		).toBe(false);
		expect(
			issueCodes(
				validatePortCompatibility(
					{ ...source.outputs[0], direction: 'input' },
					transform.inputs[0],
					{ registry }
				)
			)
		).toContain('invalid_port_direction');
	});
});

describe('workflow commands and history', () => {
	it('mutates nodes, layout, configuration, and connections through validated commands', () => {
		const registry = fixtureRegistry();
		let document = createEmptyWorkflow('commands', 'Commands');
		const source = fixtureNode('source', 'fixture.source');
		const transform = fixtureNode('transform', 'fixture.transform');

		document = addNode(document, source, { x: 10, y: 20 }, registry);
		document = addNode(document, transform, { x: 300, y: 20 }, registry);
		document = addConnection(
			document,
			fixtureConnection('source-transform', 'source', 'output', 'transform', 'input'),
			registry
		);
		document = updateNodeConfig(document, 'transform', { deduplicate: true }, registry);
		document = updateNodePosition(document, 'transform', { x: 320, y: 40 }, registry);

		expect(document.nodes.find((node) => node.id === transform.id)?.config).toEqual({
			deduplicate: true
		});
		expect(document.layout?.nodes.transform).toEqual({ x: 320, y: 40 });

		const removed = removeNode(document, source.id, registry);
		expect(removed.connections).toEqual([]);
		expect(removed.layout?.nodes.source).toBeUndefined();
		expect(removeNode(workflow([source]), source.id, registry)).toEqual(workflow([]));

		const rejected = applyWorkflowCommand(
			document,
			workflowCommands.addConnection(
				fixtureConnection('wrong-type', 'source', 'output', 'review', 'input')
			),
			registry
		);
		expect(rejected.ok).toBe(false);
	});

	it('supports undo and redo while preserving immutable snapshots', () => {
		const registry = fixtureRegistry();
		const initial = createEmptyWorkflow('history', 'History');
		const history = createWorkflowHistory(initial, { registry });
		const source = fixtureNode('source', 'fixture.source');

		const added = history.execute(workflowCommands.addNode(source, { x: 1, y: 2 }));
		expect(added.ok).toBe(true);
		expect(history.canUndo).toBe(true);
		expect(history.workflow.nodes).toHaveLength(1);

		const snapshot = history.workflow;
		snapshot.nodes.pop();
		expect(history.workflow.nodes).toHaveLength(1);

		expect(history.undo().ok).toBe(true);
		expect(history.workflow.nodes).toHaveLength(0);
		expect(history.redo().ok).toBe(true);
		expect(history.workflow.nodes).toHaveLength(1);
	});
});

describe('legacy v1 migration', () => {
	it('migrates the real v1 graph envelope into canonical typed connections', () => {
		const legacy = {
			version: 1,
			updatedAt: '2026-01-01T00:00:00Z',
			nodes: [
				{
					id: 'source',
					type: 'automation',
					position: { x: 10, y: 20 },
					data: {
						kind: 'source',
						key: 'fixture.source',
						label: 'Source',
						description: 'Fixture source',
						config: { query: 'trial' }
					}
				},
				{
					id: 'transform',
					type: 'automation',
					position: { x: 320, y: 20 },
					data: {
						kind: 'transform',
						key: 'fixture.transform',
						label: 'Transform',
						description: 'Fixture transform',
						config: { deduplicate: true }
					}
				}
			],
			edges: [
				{
					id: 'source-transform',
					source: 'source',
					target: 'transform',
					sourceHandle: 'output',
					targetHandle: 'input',
					label: 'papers'
				}
			]
		};
		const registry = fixtureRegistry();

		const migrated = migrateLegacyAutomationGraph(legacy, {
			id: 'legacy-fixture',
			name: 'Migrated fixture',
			registry
		});

		expect(migrated.ok).toBe(true);
		if (!migrated.ok) return;
		expect(migrated.fromVersion).toBe(1);
		expect(migrated.workflow.id).toBe('legacy-fixture');
		expect(migrated.workflow.nodes[0]?.kind).toBe('fixture.source');
		expect(migrated.workflow.nodes[0]?.metadata).toEqual({
			label: 'Source',
			description: 'Fixture source'
		});
		expect(migrated.workflow.layout?.nodes.source).toEqual({ x: 10, y: 20 });
		expect(migrated.workflow.connections[0]?.source.portId).toBe('output');
		expect(migrated.workflow.connections[0]?.metadata).toEqual({ label: 'papers' });

		const parsed = deserializeWorkflowDefinition(JSON.stringify(legacy), {
			registry,
			legacy: { id: 'legacy-fixture', name: 'Migrated fixture', registry }
		});
		expect(parsed.success).toBe(true);
		if (parsed.success) expect(parsed.migrated).toBe(true);
	});

	it('migrates only version one and protects invalid legacy input', () => {
		const legacy = { version: 2, nodes: [], edges: [] };
		const malformed = '{"version":1,"nodes":[{}],"edges":[]}';
		const accessorEnvelope: Record<string, unknown> = {
			version: 1,
			nodes: [],
			edges: []
		};
		Object.defineProperty(accessorEnvelope, 'updatedAt', {
			enumerable: true,
			get: () => 'not plain JSON'
		});

		expect(isLegacyAutomationGraph(legacy)).toBe(false);
		expect(isLegacyAutomationGraph(accessorEnvelope)).toBe(false);
		expect(migrateLegacyAutomationGraph(legacy).ok).toBe(false);
		const result = deserializeWorkflowDefinition(malformed, {
			allowLegacyAutomationGraph: true
		});
		expect(result.success).toBe(false);
		if (!result.success) {
			expect(result.raw).toBe(malformed);
			expect(result.issues.map((issue) => issue.code)).toContain('legacy_migration_failed');
		}
	});

	it('can disable legacy migration explicitly', () => {
		const legacy = JSON.stringify({ version: 1, nodes: [], edges: [] });
		const result = deserializeWorkflowDefinition(legacy, {
			allowLegacyAutomationGraph: false
		});

		expect(result.success).toBe(false);
		if (!result.success) {
			expect(result.raw).toBe(legacy);
			expect(result.issues.map((issue) => issue.code)).toContain('legacy_migration_failed');
		}
	});
});

describe('stable identifier constructors', () => {
	it('accept valid identifiers and reject values that cannot be persisted safely', () => {
		expect(createWorkflowId('workflow-1')).toBe('workflow-1');
		expect(createWorkflowNodeId('node-1')).toBe('node-1');
		expect(createWorkflowPortId('output.main')).toBe('output.main');
		expect(createWorkflowConnectionId('edge:1')).toBe('edge:1');
		expect(createWorkflowNodeKind('fixture.source')).toBe('fixture.source');
		expect(createWorkflowNodeId(' node')).toBeNull();
		expect(createWorkflowId('workflow/1')).toBeNull();
		expect(createWorkflowPortId('')).toBeNull();
		expect(createWorkflowConnectionId('edge/1')).toBeNull();
		expect(createWorkflowNodeKind(42)).toBeNull();
	});
});
