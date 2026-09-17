import { WorkflowNodeRegistry } from '../workflows/domain/registry';
import {
	createWorkflowNodeKind,
	workflowId,
	workflowNodeId,
	workflowPortId,
	workflowConnectionId,
	type WorkflowDefinition,
	type WorkflowNodeDefinition,
	type WorkflowNodeKind
} from '../workflows/domain/types';

function previewKind(value: string): WorkflowNodeKind {
	const kind = createWorkflowNodeKind(value);
	if (!kind) throw new Error('Invalid assistant preview kind');
	return kind;
}

const kinds = {
	context: previewKind('assistant.context'),
	tool: previewKind('assistant.tool'),
	review: previewKind('assistant.review'),
	result: previewKind('assistant.result')
};

function definition(
	kind: WorkflowNodeKind,
	title: string,
	category: string,
	input: boolean,
	output: boolean
): WorkflowNodeDefinition {
	return {
		kind,
		version: 1,
		title,
		category,
		inputs: input
			? [
					{
						id: workflowPortId('input'),
						direction: 'input',
						dataType: 'core.json',
						label: 'Context',
						cardinality: 'single'
					}
				]
			: [],
		outputs: output
			? [
					{
						id: workflowPortId('output'),
						direction: 'output',
						dataType: 'core.json',
						label: 'Result',
						cardinality: 'many'
					}
				]
			: [],
		validateConfig: (config) =>
			typeof config === 'object' &&
			config !== null &&
			!Array.isArray(config) &&
			Object.keys(config).length === 0
				? { valid: true, issues: [] }
				: {
						valid: false,
						issues: [
							{
								code: 'invalid_node_config',
								message:
									'Assistant previews do not accept executable configuration.'
							}
						]
					}
	};
}

/** A read-only explanation of a tool's authority, never an executable recipe. */
const ASSISTANT_PREVIEW_DEFINITIONS: readonly WorkflowNodeDefinition[] = [
	definition(kinds.context, 'Project context', 'Trigger', false, true),
	definition(kinds.tool, 'Assistant tool', 'Agent', true, true),
	definition(kinds.review, 'Human review', 'Human review', true, false),
	definition(kinds.result, 'Tool result', 'Result', true, false)
];

export const assistantPreviewRegistry = new WorkflowNodeRegistry(ASSISTANT_PREVIEW_DEFINITIONS);

export function createAssistantPreview({
	projectId,
	label,
	description,
	proposal
}: {
	projectId: string;
	label: string;
	description: string;
	proposal: boolean;
}): WorkflowDefinition {
	const scope = workflowNodeId('node-scope');
	const tool = workflowNodeId('node-agent');
	const terminal = workflowNodeId('node-terminal');
	return {
		schemaVersion: 1,
		id: workflowId('assistant-preview'),
		name: 'Assistant tool preview',
		nodes: [
			{
				id: scope,
				kind: kinds.context,
				definitionVersion: 1,
				config: {},
				metadata: {
					label: 'Project Context Trigger',
					description: `Project Scope: ${projectId.slice(0, 8)}…`
				}
			},
			{
				id: tool,
				kind: kinds.tool,
				definitionVersion: 1,
				config: {},
				metadata: { label, description }
			},
			{
				id: terminal,
				kind: proposal ? kinds.review : kinds.result,
				definitionVersion: 1,
				config: {},
				metadata: {
					label: proposal ? 'Human Gate Review' : 'Verified Evidence Stream',
					description: proposal
						? 'Queues safe reviewer work without changing data state'
						: 'Renders verified project telemetry & metrics'
				}
			}
		],
		connections: [
			{
				id: workflowConnectionId('e-scope-agent'),
				source: { nodeId: scope, portId: workflowPortId('output') },
				target: { nodeId: tool, portId: workflowPortId('input') }
			},
			{
				id: workflowConnectionId('e-agent-terminal'),
				source: { nodeId: tool, portId: workflowPortId('output') },
				target: { nodeId: terminal, portId: workflowPortId('input') }
			}
		],
		layout: {
			nodes: {
				[scope]: { x: 40, y: 110 },
				[tool]: { x: 380, y: 110 },
				[terminal]: { x: 740, y: 110 }
			}
		}
	};
}
