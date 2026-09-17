import { ClassicPreset, type GetSchemes } from 'rete';

import type {
	WorkflowConnection,
	WorkflowDataTypeId,
	WorkflowNode,
	WorkflowNodeDefinition,
	WorkflowNodeId,
	WorkflowNodeKind,
	WorkflowPortDefinition,
	WorkflowPortDirection
} from '../domain/types';

export class WorkflowSocket extends ClassicPreset.Socket {
	readonly workflowPortId: string;
	readonly workflowDataType: WorkflowDataTypeId;
	readonly workflowDirection: WorkflowPortDirection;
	readonly workflowLabel: string;

	constructor(port: WorkflowPortDefinition) {
		super(port.label ?? String(port.id));
		this.workflowPortId = String(port.id);
		this.workflowDataType = port.dataType;
		this.workflowDirection = port.direction;
		this.workflowLabel = port.label ?? String(port.id);
	}
}

export class WorkflowConfigControl extends ClassicPreset.Control {
	readonly workflowConfigKey: string;
	readonly workflowConfigType: 'text' | 'number' | 'boolean';
	readonly workflowReadonly: boolean;
	value: string | number | boolean;
	private readonly onValueChange: (value: string | number | boolean) => void;

	constructor({
		key,
		value,
		readonly = false,
		onChange
	}: {
		key: string;
		value: string | number | boolean;
		readonly?: boolean;
		onChange: (value: string | number | boolean) => void;
	}) {
		super();
		this.workflowConfigKey = key;
		this.workflowConfigType =
			typeof value === 'string' ? 'text' : typeof value === 'number' ? 'number' : 'boolean';
		this.workflowReadonly = readonly;
		this.value = value;
		this.onValueChange = onChange;
	}

	setValue(value: string | number | boolean): void {
		if (this.workflowReadonly) return;
		this.value = value;
		this.onValueChange(value);
	}
}

export class WorkflowReteNode extends ClassicPreset.Node {
	/**
	 * The minimap plugin needs stable dimensions before a browser layout pass.
	 * The area renderer can still display the node at its intrinsic CSS size;
	 * these values provide a useful initial overview rectangle.
	 */
	readonly width = 272;
	readonly height = 176;
	readonly workflowNodeId: WorkflowNodeId;
	workflowKind: WorkflowNodeKind;
	workflowDefinitionVersion: number;
	workflowNode: WorkflowNode;
	workflowTitle: string;
	workflowDescription: string | undefined;
	workflowCategory: string;
	workflowUnsupported: boolean;

	constructor(node: WorkflowNode, definition: WorkflowNodeDefinition | undefined) {
		super(definition?.title ?? String(node.kind));
		this.id = node.id;
		this.workflowNodeId = node.id;
		this.workflowKind = node.kind;
		this.workflowDefinitionVersion = node.definitionVersion;
		this.workflowNode = node;
		this.workflowTitle = definition?.title ?? String(node.kind);
		this.workflowDescription = definition?.description;
		this.workflowCategory = definition?.category ?? 'Unsupported';
		this.workflowUnsupported = definition === undefined;
	}
}

export type WorkflowReteConnection = ClassicPreset.Connection<
	WorkflowReteNode,
	WorkflowReteNode
> & {
	readonly workflowConnectionId: string;
	readonly workflowConnection: WorkflowConnection;
};

export type WorkflowSchemes = GetSchemes<WorkflowReteNode, WorkflowReteConnection>;

export function createWorkflowReteConnection(
	connection: WorkflowConnection,
	source: WorkflowReteNode,
	target: WorkflowReteNode
): WorkflowReteConnection {
	const reteConnection = new ClassicPreset.Connection(
		source,
		String(connection.source.portId),
		target,
		String(connection.target.portId)
	) as WorkflowReteConnection;
	reteConnection.id = connection.id;
	Object.defineProperty(reteConnection, 'workflowConnectionId', {
		configurable: false,
		enumerable: false,
		value: String(connection.id),
		writable: false
	});
	Object.defineProperty(reteConnection, 'workflowConnection', {
		configurable: false,
		enumerable: false,
		value: connection,
		writable: false
	});
	return reteConnection;
}
