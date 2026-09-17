import { ClassicPreset, NodeEditor, type Root } from 'rete';
import {
	AreaExtensions,
	AreaPlugin,
	type Area2D,
	type Position as AreaPosition
} from 'rete-area-plugin';
import {
	ClassicFlow,
	ConnectionPlugin,
	type Context,
	type SocketData
} from 'rete-connection-plugin';
import { MinimapPlugin, type MinimapExtra } from 'rete-minimap-plugin';
import { Presets, SveltePlugin, type SvelteArea2D } from 'rete-svelte-plugin/5';

import { DEFAULT_WORKFLOW_REGISTRY, WorkflowNodeRegistry } from '../domain/registry';
import { applyWorkflowCommand, type WorkflowCommand } from '../domain/commands';
import { validateWorkflow } from '../domain/validation';
import {
	createWorkflowReteConnection,
	WorkflowConfigControl,
	WorkflowReteNode,
	WorkflowSocket,
	type WorkflowReteConnection,
	type WorkflowSchemes
} from './rete-types';
import WorkflowConnection from './WorkflowConnection.svelte';
import WorkflowControl from './WorkflowControl.svelte';
import WorkflowNodeView from './WorkflowNode.svelte';
import WorkflowSocketView from './WorkflowSocket.svelte';
import './workflow-editor.css';
import type {
	JsonObject,
	JsonValue,
	WorkflowConnection as WorkflowIRConnection,
	WorkflowDefinition,
	WorkflowNode,
	WorkflowNodeId,
	WorkflowValidationIssue
} from '../domain/types';
import {
	createWorkflowConnectionId,
	createWorkflowNodeId,
	createWorkflowPortId
} from '../domain/types';

type SvelteAreaExtra = SvelteArea2D<WorkflowSchemes>;
type AreaExtra = SvelteAreaExtra | MinimapExtra;
type AreaSignal = Area2D<WorkflowSchemes> | AreaExtra | Root<WorkflowSchemes>;
type WorkflowConnectionScopeSignals = [];
type WorkflowConnectionContext = Pick<
	Context<WorkflowSchemes, WorkflowConnectionScopeSignals>,
	'editor'
>;

export type WorkflowEditorChangeType =
	| 'node-added'
	| 'node-removed'
	| 'node-moved'
	| 'node-configured'
	| 'connection-added'
	| 'connection-removed';

export interface WorkflowEditorChange {
	readonly type: WorkflowEditorChangeType;
	readonly workflow: WorkflowDefinition;
	readonly nodeId?: WorkflowNodeId;
	readonly connectionId?: string;
}

export class WorkflowEditorError extends Error {
	readonly issues: readonly WorkflowValidationIssue[];

	constructor(message: string, issues: readonly WorkflowValidationIssue[] = []) {
		super(message);
		this.name = 'WorkflowEditorError';
		this.issues = issues;
	}
}

export interface WorkflowEditorOptions {
	readonly container: HTMLElement;
	readonly workflow: WorkflowDefinition;
	readonly registry?: WorkflowNodeRegistry;
	readonly readOnly?: boolean;
	readonly onChange?: (change: WorkflowEditorChange) => void;
	readonly onSelectionChange?: (nodeIds: readonly WorkflowNodeId[]) => void;
	readonly onViewportChange?: (viewport: WorkflowEditorViewport) => void;
	readonly onError?: (error: WorkflowEditorError) => void;
}

export interface WorkflowEditorViewport {
	readonly zoomPercent: number;
	readonly x: number;
	readonly y: number;
}

export interface WorkflowEditorHandle {
	getWorkflow(): WorkflowDefinition;
	getViewport(): WorkflowEditorViewport;
	reconcile(workflow: WorkflowDefinition): Promise<void>;
	addNode(node: WorkflowDefinition['nodes'][number]): Promise<boolean>;
	removeNode(nodeId: WorkflowNodeId): Promise<boolean>;
	removeConnection(connectionId: string): Promise<boolean>;
	setZoomPercent(percent: number): Promise<boolean>;
	zoomIn(): Promise<boolean>;
	zoomOut(): Promise<boolean>;
	fitView(): Promise<void>;
	selectNode(nodeId: WorkflowNodeId | null): Promise<void>;
	destroy(): void;
}

/**
 * Create a Rete-backed authoring surface for a renderer-neutral workflow.
 *
 * This module is intentionally imported dynamically by WorkflowEditor.svelte.
 * Rete's area/renderer packages touch browser APIs while being evaluated, so
 * keeping the import at the client lifecycle boundary is part of the adapter
 * contract.
 */
export async function createWorkflowEditor(
	options: WorkflowEditorOptions
): Promise<WorkflowEditorHandle> {
	const registry = options.registry ?? DEFAULT_WORKFLOW_REGISTRY;
	const validation = validateWorkflow(options.workflow, registry);
	if (!validation.valid) {
		throw new WorkflowEditorError('Cannot open an invalid workflow.', validation.issues);
	}

	const controller = new ReteWorkflowEditor({ ...options, registry });
	try {
		await controller.initialize();
		return controller;
	} catch (cause) {
		// Initialization can fail after one of the plugins has attached listeners.
		// Always tear down the partially constructed editor before surfacing the
		// error so a remount cannot retain stale global pointer listeners.
		controller.destroy();
		throw cause;
	}
}

class ReteWorkflowEditor implements WorkflowEditorHandle {
	readonly editor: NodeEditor<WorkflowSchemes>;
	readonly area: AreaPlugin<WorkflowSchemes, AreaExtra>;

	private readonly container: HTMLElement;
	private readonly registry: WorkflowNodeRegistry;
	private readonly readOnly: boolean;
	private readonly onChange?: (change: WorkflowEditorChange) => void;
	private readonly onSelectionChange?: (nodeIds: readonly WorkflowNodeId[]) => void;
	private readonly onViewportChange?: (viewport: WorkflowEditorViewport) => void;
	private readonly onError?: (error: WorkflowEditorError) => void;
	private readonly selector = AreaExtensions.selector();
	private readonly accumulating = AreaExtensions.accumulateOnCtrl();
	private readonly connection: ConnectionPlugin<WorkflowSchemes, AreaExtra>;
	private readonly minimap: MinimapPlugin<WorkflowSchemes>;
	private readonly render: SveltePlugin<WorkflowSchemes, AreaExtra>;
	private workflow: WorkflowDefinition;
	private operationQueue: Promise<void> = Promise.resolve();
	private lastEmittedSelection: readonly WorkflowNodeId[] | null = null;
	private reconciling = 0;
	private active = true;
	private initialized = false;

	constructor(options: WorkflowEditorOptions & { readonly registry: WorkflowNodeRegistry }) {
		this.container = options.container;
		this.registry = options.registry;
		this.readOnly = options.readOnly === true;
		this.onChange = options.onChange;
		this.onSelectionChange = options.onSelectionChange;
		this.onViewportChange = options.onViewportChange;
		this.onError = options.onError;
		this.workflow = cloneWorkflow(options.workflow);
		this.editor = new NodeEditor<WorkflowSchemes>();
		this.area = new AreaPlugin<WorkflowSchemes, AreaExtra>(this.container);
		this.connection = new ConnectionPlugin<WorkflowSchemes, AreaExtra>();
		this.minimap = new MinimapPlugin({ boundViewport: true, minDistance: 2000, ratio: 1 });
		this.render = new SveltePlugin<WorkflowSchemes, AreaExtra>();
		this.area.area.content.holder.dataset.workflowViewport = '';
		this.area.area.content.holder.classList.add('workflow-rete-viewport');

		this.editor.addPipe((signal) => this.handleEditorSignal(signal));
	}

	async initialize(): Promise<void> {
		if (!this.active || this.initialized) return;

		try {
			// The area is the bridge between the editor's root signals and every
			// renderer/interaction plugin. Without this parent relationship, the
			// plugins can be constructed but never receive node or pointer events.
			this.editor.use(this.area);
			this.area.use(this.connection);
			this.area.use(this.minimap);
			this.render.addPreset(
				Presets.classic.setup({
					customize: {
						node: () => WorkflowNodeView,
						connection: () => WorkflowConnection,
						socket: () => WorkflowSocketView,
						control: (context) =>
							context.payload instanceof WorkflowConfigControl
								? WorkflowControl
								: null
					}
				})
			);
			this.render.addPreset(Presets.minimap.setup({ size: 176 }));
			this.area.use(this.render);
			AreaExtensions.restrictor(this.area, { scaling: { min: 0.5, max: 2 } });
			AreaExtensions.zIndexNodesOrder(this.area);
			AreaExtensions.selectableNodes(this.area, this.selector, {
				accumulating: this.accumulating
			});
			// Register after selectableNodes so nodepicked/pointerup callbacks see
			// the selector's updated state, including background unselection and
			// ctrl/meta multi-selection.
			this.area.addPipe((signal) => this.handleAreaSignal(signal));
			this.minimap.element.classList.add('workflow-rete-minimap');
			this.minimap.element.classList.toggle('workflow-rete-minimap-readonly', this.readOnly);
			this.connection.addPreset(
				() =>
					new ClassicFlow<WorkflowSchemes, WorkflowConnectionScopeSignals>({
						canMakeConnection: (from, to) => this.canMakeConnection(from, to),
						makeConnection: (from, to, context) =>
							this.makeConnection(from, to, context)
					})
			);

			await this.reconcile(this.workflow);
			this.initialized = true;
			this.emitSelection();
			this.emitViewport();
		} catch (cause) {
			this.destroy();
			throw cause;
		}
	}

	getWorkflow(): WorkflowDefinition {
		return cloneWorkflow(this.workflow);
	}

	getViewport(): WorkflowEditorViewport {
		return {
			zoomPercent: clamp(Math.round(this.area.area.transform.k * 100), 50, 200),
			x: this.area.area.transform.x,
			y: this.area.area.transform.y
		};
	}

	reconcile(nextWorkflow: WorkflowDefinition): Promise<void> {
		return this.enqueue(() => this.reconcileInternal(nextWorkflow));
	}

	private async reconcileInternal(nextWorkflow: WorkflowDefinition): Promise<void> {
		if (!this.active) return;
		const validation = validateWorkflow(nextWorkflow, this.registry);
		if (!validation.valid) {
			const error = new WorkflowEditorError(
				'Cannot reconcile an invalid workflow.',
				validation.issues
			);
			this.reportError(error);
			throw error;
		}

		const next = cloneWorkflow(nextWorkflow);
		const previous = this.workflow;
		const selectedBefore = this.selectedNodeIds();
		this.reconciling += 1;
		try {
			this.workflow = next;
			const nextConnections = new Map(
				next.connections.map((value) => [String(value.id), value])
			);
			for (const current of this.editor.getConnections()) {
				const desired = nextConnections.get(String(current.id));
				if (!desired || !sameReteConnection(current, desired)) {
					const removed = await this.editor.removeConnection(current.id);
					if (!removed && this.editor.getConnection(current.id)) {
						throw new WorkflowEditorError(
							`Could not remove workflow connection ${String(current.id)}.`
						);
					}
					if (!this.active) return;
				}
			}

			const nextNodes = new Map(next.nodes.map((value) => [String(value.id), value]));
			for (const current of this.editor.getNodes()) {
				if (!nextNodes.has(String(current.id))) {
					await this.removeReteNode(current.id);
					if (!this.active) return;
				}
			}

			for (const node of next.nodes) {
				const current = this.editor.getNode(node.id);
				if (!current) {
					await this.addReteNode(node);
					if (!this.active) return;
				} else if (nodeNeedsRebuild(current, node, this.registry)) {
					await this.removeReteNode(current.id);
					if (!this.active) return;
					await this.addReteNode(node);
					if (!this.active) return;
				} else {
					if (updateReteNode(current, node, this.registry)) {
						await this.area.update('node', node.id);
						if (!this.active) return;
					}
				}
			}

			const currentConnections = new Map(
				this.editor.getConnections().map((value) => [String(value.id), value])
			);
			for (const desired of next.connections) {
				if (currentConnections.has(String(desired.id))) continue;
				const source = this.editor.getNode(desired.source.nodeId);
				const target = this.editor.getNode(desired.target.nodeId);
				if (!source || !target) {
					throw new WorkflowEditorError(
						`Cannot hydrate connection ${String(desired.id)} because its node is missing.`
					);
				}
				const added = await this.editor.addConnection(
					createWorkflowReteConnection(desired, source, target)
				);
				if (!added) {
					throw new WorkflowEditorError(
						`Could not add workflow connection ${String(desired.id)}.`
					);
				}
				if (!this.active) return;
			}

			for (const node of next.nodes) {
				const layout = next.layout?.nodes[String(node.id)];
				const view = this.area.nodeViews.get(node.id);
				if (!layout || !view) continue;
				if (view.position.x !== layout.x || view.position.y !== layout.y) {
					await this.area.translate(node.id, { x: layout.x, y: layout.y });
					if (!this.active) return;
				}
			}

			await this.reconcileSelection(
				selectedBefore.filter((nodeId) => nextNodes.has(String(nodeId)))
			);
			if (!this.active) return;
			this.emitSelection();
		} catch (cause) {
			this.workflow = previous;
			const error =
				cause instanceof WorkflowEditorError
					? cause
					: new WorkflowEditorError('Rete could not hydrate the workflow.');
			this.reportError(error);
			throw error;
		} finally {
			this.reconciling -= 1;
		}
	}

	addNode(node: WorkflowDefinition['nodes'][number]): Promise<boolean> {
		return this.enqueue(() => this.addNodeInternal(node));
	}

	private async addNodeInternal(node: WorkflowDefinition['nodes'][number]): Promise<boolean> {
		if (!this.active || this.readOnly) return false;
		const next = this.applyCommand(
			{ type: 'add-node', node },
			'Cannot add an invalid workflow node.'
		);
		if (!next) return false;
		this.reconciling += 1;
		try {
			const addedNode = next.nodes.find((candidate) => candidate.id === node.id);
			if (!addedNode) return false;
			await this.addReteNode(addedNode);
			if (!this.active) return false;
			this.workflow = cloneWorkflow(next);
			this.emitChange({ type: 'node-added', nodeId: node.id });
			return true;
		} catch (cause) {
			this.reportError(
				cause instanceof WorkflowEditorError
					? cause
					: new WorkflowEditorError('Could not add the workflow node.')
			);
			return false;
		} finally {
			this.reconciling -= 1;
		}
	}

	removeNode(nodeId: WorkflowNodeId): Promise<boolean> {
		return this.enqueue(() => this.removeNodeInternal(nodeId));
	}

	private async removeNodeInternal(nodeId: WorkflowNodeId): Promise<boolean> {
		if (!this.active || this.readOnly) return false;
		if (!this.workflow.nodes.some((node) => node.id === nodeId)) return false;
		const next = this.applyCommand(
			{ type: 'remove-node', nodeId },
			'Could not remove the workflow node.'
		);
		if (!next) return false;
		this.reconciling += 1;
		try {
			await this.removeReteNode(nodeId);
			if (!this.active) return false;
			this.workflow = cloneWorkflow(next);
			this.emitSelection();
			this.emitChange({ type: 'node-removed', nodeId });
			return true;
		} catch (cause) {
			this.reportError(
				cause instanceof WorkflowEditorError
					? cause
					: new WorkflowEditorError('Could not remove the workflow node.')
			);
			return false;
		} finally {
			this.reconciling -= 1;
		}
	}

	removeConnection(connectionId: string): Promise<boolean> {
		return this.enqueue(() => this.removeConnectionInternal(connectionId));
	}

	private async removeConnectionInternal(connectionId: string): Promise<boolean> {
		if (!this.active || this.readOnly) return false;
		const current = this.editor.getConnection(connectionId);
		if (!current) return false;
		const next = this.applyCommand(
			{ type: 'remove-connection', connectionId },
			'Could not remove the workflow connection.'
		);
		if (!next) return false;
		this.reconciling += 1;
		try {
			const removed = await this.editor.removeConnection(connectionId);
			if (!removed) return false;
			if (!this.active) return false;
			this.workflow = next;
			this.emitChange({ type: 'connection-removed', connectionId });
			return true;
		} catch (cause) {
			this.reportError(
				cause instanceof WorkflowEditorError
					? cause
					: new WorkflowEditorError('Could not remove the workflow connection.')
			);
			return false;
		} finally {
			this.reconciling -= 1;
		}
	}

	setZoomPercent(percent: number): Promise<boolean> {
		return this.enqueue(() => this.setZoomPercentInternal(percent));
	}

	private async setZoomPercentInternal(percent: number): Promise<boolean> {
		if (!this.active || !Number.isFinite(percent)) return false;
		const next = clamp(percent, 50, 200) / 100;
		const centerX = this.container.clientWidth / 2;
		const centerY = this.container.clientHeight / 2;
		await this.area.area.zoom(next, centerX, centerY);
		return this.active;
	}

	zoomIn(): Promise<boolean> {
		return this.setZoomPercent(this.getViewport().zoomPercent + 10);
	}

	zoomOut(): Promise<boolean> {
		return this.setZoomPercent(this.getViewport().zoomPercent - 10);
	}

	fitView(): Promise<void> {
		return this.enqueue(() => this.fitViewInternal());
	}

	private async fitViewInternal(): Promise<void> {
		if (!this.active) return;
		const nodes = this.editor.getNodes();
		const rects = nodes
			.map((node) => {
				const view = this.area.nodeViews.get(node.id);
				if (!view) return undefined;
				return {
					position: view.position,
					width: view.element.clientWidth,
					height: view.element.clientHeight
				};
			})
			.filter(
				(value): value is { position: AreaPosition; width: number; height: number } =>
					value !== undefined && value.width > 0 && value.height > 0
			);
		if (rects.length === 0) return;

		const bounds = AreaExtensions.getBoundingBox(this.area, nodes);
		const width = Math.max(bounds.width, 1);
		const height = Math.max(bounds.height, 1);
		const horizontal = (this.container.clientWidth * 0.9) / width;
		const vertical = (this.container.clientHeight * 0.9) / height;
		const zoom = clamp(Math.min(horizontal, vertical), 0.5, 1.1);
		await this.area.area.zoom(zoom, 0, 0);
		if (!this.active) return;
		await this.area.area.translate(
			this.container.clientWidth / 2 - bounds.center.x * zoom,
			this.container.clientHeight / 2 - bounds.center.y * zoom
		);
	}

	selectNode(nodeId: WorkflowNodeId | null): Promise<void> {
		return this.enqueue(() => this.selectNodeInternal(nodeId));
	}

	private async selectNodeInternal(nodeId: WorkflowNodeId | null): Promise<void> {
		if (!this.active) return;
		const desired = nodeId !== null && this.editor.getNode(nodeId) ? [nodeId] : [];
		await this.reconcileSelection(desired);
		if (!this.active) return;
		this.emitSelection();
	}

	destroy(): void {
		if (!this.active) return;
		this.active = false;
		this.selector.entities.clear();
		this.selector.release();
		let cleanupError: unknown;
		try {
			this.connection.drop();
		} catch (cause) {
			cleanupError ??= cause;
		}
		try {
			this.accumulating.destroy();
		} catch (cause) {
			cleanupError ??= cause;
		}
		try {
			this.area.destroy();
		} catch (cause) {
			cleanupError ??= cause;
		}
		try {
			this.minimap.element?.remove();
		} catch (cause) {
			cleanupError ??= cause;
		}
		if (cleanupError instanceof Error) {
			this.onError?.(new WorkflowEditorError('Could not destroy the workflow editor.'));
		}
	}

	private enqueue<T>(operation: () => Promise<T> | T): Promise<T> {
		const next = this.operationQueue.then(operation, operation);
		this.operationQueue = next.then(
			() => undefined,
			() => undefined
		);
		return next;
	}

	private enqueueMutation(operation: () => Promise<void> | void): void {
		void this.enqueue(operation).catch((cause) => {
			this.reportError(
				cause instanceof WorkflowEditorError
					? cause
					: new WorkflowEditorError('Could not apply the workflow editor change.')
			);
		});
	}

	/**
	 * Keep every renderer mutation on the domain command path. Rete owns the
	 * transient graph objects; this method is the only place where a renderer
	 * event is turned into persisted workflow state.
	 */
	private applyCommand(
		command: WorkflowCommand,
		message: string,
		base: WorkflowDefinition = this.workflow
	): WorkflowDefinition | null {
		const result = applyWorkflowCommand(base, command, this.registry);
		if (result.ok) return result.workflow;
		this.reportError(new WorkflowEditorError(message, result.issues));
		return null;
	}

	private handleEditorSignal(signal: Root<WorkflowSchemes>): Root<WorkflowSchemes> | undefined {
		if (!this.active) return undefined;
		if (
			this.readOnly &&
			!this.reconciling &&
			(signal.type === 'nodecreate' ||
				signal.type === 'noderemove' ||
				signal.type === 'connectioncreate' ||
				signal.type === 'connectionremove')
		) {
			return undefined;
		}
		if (signal.type === 'connectioncreate' && !this.reconciling) {
			const connection = toWorkflowConnection(signal.data);
			if (!connection) return undefined;
			const validation = validateCandidate(connection, this.workflow, this.registry);
			if (!validation.valid) {
				this.reportError(
					new WorkflowEditorError('Connection is not compatible.', validation.issues)
				);
				return undefined;
			}
		}
		if (signal.type === 'connectioncreated' && !this.reconciling) {
			const connection = toWorkflowConnection(signal.data);
			if (connection) this.enqueueMutation(() => this.commitConnection(connection));
		}
		if (signal.type === 'connectionremoved' && !this.reconciling) {
			this.enqueueMutation(() => this.commitConnectionRemoval(String(signal.data.id)));
		}
		if (signal.type === 'noderemoved' && !this.reconciling) {
			const nodeId = createWorkflowNodeId(signal.data.id);
			if (nodeId) this.enqueueMutation(() => this.commitNodeRemoval(nodeId));
		}
		return signal;
	}

	private handleAreaSignal(signal: AreaSignal): AreaSignal | undefined {
		if (!this.active) return undefined;
		if (this.readOnly && !this.reconciling && signal.type === 'nodetranslate') return undefined;
		if (signal.type === 'nodepicked' || signal.type === 'pointerup') this.emitSelection();
		if (signal.type === 'nodedragged' && !this.reconciling && !this.readOnly) {
			const nodeId = createWorkflowNodeId(signal.data.id);
			const position = nodeId ? this.area.nodeViews.get(nodeId)?.position : undefined;
			if (nodeId && position) {
				this.enqueueMutation(() => this.commitNodeMove(nodeId, position));
			}
		}
		if (signal.type === 'zoomed' || signal.type === 'translated') this.emitViewport();
		return signal;
	}

	private async addReteNode(node: WorkflowDefinition['nodes'][number]): Promise<void> {
		const definition = this.registry.get(String(node.kind), node.definitionVersion);
		const reteNode = new WorkflowReteNode(node, definition);
		for (const port of definition?.inputs ?? []) {
			reteNode.addInput(
				String(port.id),
				new ClassicPreset.Input(
					new WorkflowSocket(port),
					port.label,
					port.cardinality === 'many'
				)
			);
		}
		for (const port of definition?.outputs ?? []) {
			reteNode.addOutput(
				String(port.id),
				new ClassicPreset.Output(
					new WorkflowSocket(port),
					port.label,
					port.cardinality === 'many'
				)
			);
		}
		for (const [key, value] of Object.entries(node.config)) {
			if (!isControlValue(value)) continue;
			reteNode.addControl(
				key,
				new WorkflowConfigControl({
					key,
					value,
					readonly: this.readOnly,
					onChange: (nextValue) =>
						this.enqueueMutation(() => this.commitConfigValue(node.id, key, nextValue))
				})
			);
		}
		if (!(await this.editor.addNode(reteNode))) {
			throw new WorkflowEditorError(`Could not add workflow node ${String(node.id)}.`);
		}
	}

	private async removeReteNode(nodeId: string): Promise<void> {
		await this.selector.remove({ id: nodeId, label: 'node' });
		const connections = this.editor
			.getConnections()
			.filter((connection) => connection.source === nodeId || connection.target === nodeId);
		for (const connection of connections) {
			if (!(await this.editor.removeConnection(connection.id))) {
				throw new WorkflowEditorError(
					`Could not remove workflow connection ${String(connection.id)}.`
				);
			}
		}
		if (this.editor.getNode(nodeId) && !(await this.editor.removeNode(nodeId))) {
			throw new WorkflowEditorError(`Could not remove workflow node ${String(nodeId)}.`);
		}
	}

	private canMakeConnection(from: SocketData, to: SocketData): boolean {
		if (this.readOnly) return false;
		const candidate = toWorkflowConnectionFromSockets(from, to);
		if (!candidate) return false;
		return validateCandidate(candidate, this.workflow, this.registry).valid;
	}

	private makeConnection(
		from: SocketData,
		to: SocketData,
		context: WorkflowConnectionContext
	): boolean | undefined {
		if (this.readOnly) return undefined;
		const candidate = toWorkflowConnectionFromSockets(from, to);
		if (!candidate) return undefined;
		const candidateWorkflow = workflowForCandidate(candidate, this.workflow, this.registry);
		const validation = validateWorkflow(candidateWorkflow, this.registry);
		if (!validation.valid) {
			this.reportError(
				new WorkflowEditorError('Connection is not compatible.', validation.issues)
			);
			return undefined;
		}
		this.enqueueMutation(async () => {
			const latest = workflowForCandidate(candidate, this.workflow, this.registry);
			const latestValidation = validateWorkflow(latest, this.registry);
			if (!latestValidation.valid) {
				throw new WorkflowEditorError(
					'Connection is not compatible.',
					latestValidation.issues
				);
			}
			const source = context.editor.getNode(candidate.source.nodeId);
			const target = context.editor.getNode(candidate.target.nodeId);
			if (!source || !target) {
				throw new WorkflowEditorError(
					'Connection references a node that no longer exists.'
				);
			}
			if (
				!(await context.editor.addConnection(
					createWorkflowReteConnection(candidate, source, target)
				))
			) {
				throw new WorkflowEditorError(
					`Could not create workflow connection ${String(candidate.id)}.`
				);
			}
		});
		return true;
	}

	private commitConnection(connection: import('../domain/types').WorkflowConnection): void {
		if (!this.active) return;
		// `workflowForCandidate` is the validated preview used by the renderer,
		// so it already contains the candidate connection. Remove that preview
		// entry before applying the add command or the command would reject its
		// own connection as a duplicate.
		const candidateWorkflow = workflowForCandidate(connection, this.workflow, this.registry);
		const next = this.applyCommand(
			{ type: 'add-connection', connection },
			'Rete produced an invalid connection.',
			{
				...candidateWorkflow,
				connections: candidateWorkflow.connections.filter(
					(value) => String(value.id) !== String(connection.id)
				)
			}
		);
		if (!next) return;
		this.workflow = next;
		this.emitChange({ type: 'connection-added', connectionId: String(connection.id) });
	}

	private commitConnectionRemoval(connectionId: string): void {
		if (!this.active) return;
		if (!this.workflow.connections.some((value) => String(value.id) === connectionId)) return;
		const next = this.applyCommand(
			{ type: 'remove-connection', connectionId },
			'Could not remove the workflow connection.'
		);
		if (!next) return;
		this.workflow = next;
		this.emitChange({ type: 'connection-removed', connectionId });
	}

	private commitNodeRemoval(nodeId: WorkflowNodeId): void {
		if (!this.active) return;
		if (!this.workflow.nodes.some((value) => value.id === nodeId)) return;
		const next = this.applyCommand(
			{ type: 'remove-node', nodeId },
			'Could not remove the workflow node.'
		);
		if (!next) return;
		this.workflow = next;
		this.emitChange({ type: 'node-removed', nodeId });
	}

	private commitNodeMove(nodeId: WorkflowNodeId, position: AreaPosition): void {
		if (!this.active) return;
		// Rete emits one drag signal for the picked node while the selectable
		// extension translates the rest of the selection. Persist every selected
		// view in one shared command snapshot so a later reconcile does not snap
		// the other nodes back to their old layout.
		const movedNodeIds = new Set(this.selectedNodeIds());
		movedNodeIds.add(nodeId);
		let next = this.workflow;
		let changed = false;
		for (const movedNodeId of movedNodeIds) {
			const movedPosition =
				movedNodeId === nodeId ? position : this.area.nodeViews.get(movedNodeId)?.position;
			if (!movedPosition) continue;
			const candidate = this.applyCommand(
				{ type: 'update-node-position', nodeId: movedNodeId, position: movedPosition },
				'Could not update the workflow node position.',
				next
			);
			if (!candidate) return;
			changed ||= JSON.stringify(candidate.layout) !== JSON.stringify(next.layout);
			next = candidate;
		}
		if (!changed) return;
		this.workflow = next;
		this.emitChange({ type: 'node-moved', nodeId });
	}

	private commitConfigValue(
		nodeId: WorkflowNodeId,
		key: string,
		value: string | number | boolean
	): void {
		if (this.readOnly || !this.active) return;
		const node = this.workflow.nodes.find((candidate) => candidate.id === nodeId);
		if (!node) return;
		const next = this.applyCommand(
			{ type: 'update-node-config', nodeId, config: { [key]: value } },
			'Configuration is invalid.'
		);
		if (!next) {
			const control = this.editor.getNode(nodeId)?.controls[key];
			if (control instanceof WorkflowConfigControl) {
				const previousValue = node.config[key];
				if (isControlValue(previousValue)) {
					control.value = previousValue;
					void this.area.update('node', nodeId);
				}
			}
			return;
		}
		this.workflow = next;
		const reteNode = this.editor.getNode(nodeId);
		if (reteNode) {
			reteNode.workflowNode = next.nodes.find((candidate) => candidate.id === nodeId) ?? node;
		}
		this.emitChange({ type: 'node-configured', nodeId });
	}

	private setNodeSelected(nodeId: WorkflowNodeId, selected: boolean): void {
		if (!this.active) return;
		const node = this.editor.getNode(nodeId);
		if (!node) return;
		node.selected = selected;
		void this.area.update('node', nodeId);
	}

	private async translateNode(nodeId: WorkflowNodeId, dx: number, dy: number): Promise<void> {
		if (!this.active) return;
		const view = this.area.nodeViews.get(nodeId);
		if (!view) return;
		await this.area.translate(nodeId, {
			x: view.position.x + dx,
			y: view.position.y + dy
		});
	}

	private selectionEntity(nodeId: WorkflowNodeId): {
		label: 'node';
		id: WorkflowNodeId;
		unselect: () => void;
		translate: (dx: number, dy: number) => Promise<void>;
	} {
		return {
			id: nodeId,
			label: 'node',
			unselect: () => this.setNodeSelected(nodeId, false),
			translate: (dx, dy) => this.translateNode(nodeId, dx, dy)
		};
	}

	private selectedNodeIds(): WorkflowNodeId[] {
		const selected = new Set<string>();
		for (const entity of this.selector.entities.values()) {
			if (entity.label === 'node') selected.add(entity.id);
		}
		return this.editor
			.getNodes()
			.filter((node) => selected.has(String(node.id)))
			.map((node) => createWorkflowNodeId(node.id))
			.filter((nodeId): nodeId is WorkflowNodeId => nodeId !== null);
	}

	private async reconcileSelection(nodeIds: Iterable<WorkflowNodeId>): Promise<void> {
		const desired = new Set<WorkflowNodeId>();
		for (const nodeId of nodeIds) {
			if (this.editor.getNode(nodeId)) desired.add(nodeId);
		}

		for (const entity of [...this.selector.entities.values()]) {
			if (entity.label !== 'node') continue;
			const nodeId = createWorkflowNodeId(entity.id);
			if (!nodeId || !desired.has(nodeId)) {
				await this.selector.remove(entity);
			}
		}

		for (const node of this.editor.getNodes()) {
			const nodeId = createWorkflowNodeId(node.id);
			if (nodeId && !desired.has(nodeId) && node.selected)
				this.setNodeSelected(nodeId, false);
		}

		for (const nodeId of desired) {
			const entity = { id: nodeId, label: 'node' as const };
			if (!this.selector.isSelected(entity)) {
				await this.selector.add(this.selectionEntity(nodeId), true);
			}
			if (this.editor.getNode(nodeId)?.selected !== true) this.setNodeSelected(nodeId, true);
		}
	}

	private emitSelection(): void {
		if (!this.active || !this.onSelectionChange) return;
		const selected = this.selectedNodeIds();
		if (this.lastEmittedSelection && sameWorkflowNodeIds(this.lastEmittedSelection, selected))
			return;
		this.lastEmittedSelection = selected;
		this.onSelectionChange(selected);
	}

	private emitViewport(): void {
		if (!this.active) return;
		this.onViewportChange?.(this.getViewport());
	}

	private emitChange(change: Omit<WorkflowEditorChange, 'workflow'>): void {
		if (!this.active) return;
		this.onChange?.({ ...change, workflow: this.getWorkflow() });
	}

	private reportError(error: WorkflowEditorError): void {
		if (this.active) this.onError?.(error);
	}
}

function cloneWorkflow(workflow: WorkflowDefinition): WorkflowDefinition {
	return {
		schemaVersion: workflow.schemaVersion,
		id: workflow.id,
		name: workflow.name,
		...(workflow.description !== undefined ? { description: workflow.description } : {}),
		nodes: workflow.nodes.map(cloneWorkflowNode),
		connections: workflow.connections.map(cloneWorkflowConnection),
		...(workflow.layout
			? {
					layout: {
						nodes: Object.fromEntries(
							Object.entries(workflow.layout.nodes).map(([id, position]) => [
								id,
								{ ...position }
							])
						)
					}
				}
			: {}),
		...(workflow.metadata !== undefined ? { metadata: cloneJsonObject(workflow.metadata) } : {})
	};
}

function cloneWorkflowNode(node: WorkflowNode): WorkflowNode {
	return {
		id: node.id,
		kind: node.kind,
		definitionVersion: node.definitionVersion,
		config: cloneJsonObject(node.config),
		...(node.label !== undefined ? { label: node.label } : {}),
		...(node.metadata !== undefined ? { metadata: cloneJsonObject(node.metadata) } : {})
	};
}

function cloneWorkflowConnection(connection: WorkflowIRConnection): WorkflowIRConnection {
	return {
		id: connection.id,
		source: { nodeId: connection.source.nodeId, portId: connection.source.portId },
		target: { nodeId: connection.target.nodeId, portId: connection.target.portId },
		...(connection.metadata !== undefined
			? { metadata: cloneJsonObject(connection.metadata) }
			: {})
	};
}

function cloneJsonObject(value: JsonObject): JsonObject {
	const clone: Record<string, JsonValue> = {};
	for (const [key, child] of Object.entries(value)) clone[key] = cloneJsonValue(child);
	return clone;
}

function cloneJsonValue(value: JsonValue): JsonValue {
	if (Array.isArray(value)) return value.map(cloneJsonValue);
	if (value !== null && typeof value === 'object') return cloneJsonObject(value);
	return value;
}

function isControlValue(
	value: import('../domain/types').JsonValue
): value is string | number | boolean {
	return typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean';
}

function toWorkflowConnection(
	value: WorkflowReteConnection
): import('../domain/types').WorkflowConnection | null {
	const id = createWorkflowConnectionId(String(value.id));
	const sourceNodeId = createWorkflowNodeId(value.source);
	const sourcePortId = createWorkflowPortId(String(value.sourceOutput));
	const targetNodeId = createWorkflowNodeId(value.target);
	const targetPortId = createWorkflowPortId(String(value.targetInput));
	if (!id || !sourceNodeId || !sourcePortId || !targetNodeId || !targetPortId) return null;
	return {
		id,
		source: { nodeId: sourceNodeId, portId: sourcePortId },
		target: { nodeId: targetNodeId, portId: targetPortId }
	};
}

function toWorkflowConnectionFromSockets(
	from: SocketData,
	to: SocketData
): import('../domain/types').WorkflowConnection | null {
	const source = from.side === 'output' ? from : to.side === 'output' ? to : undefined;
	const target = from.side === 'input' ? from : to.side === 'input' ? to : undefined;
	if (!source || !target) return null;
	const sourceNodeId = createWorkflowNodeId(source.nodeId);
	const sourcePortId = createWorkflowPortId(source.key);
	const targetNodeId = createWorkflowNodeId(target.nodeId);
	const targetPortId = createWorkflowPortId(target.key);
	if (!sourceNodeId || !sourcePortId || !targetNodeId || !targetPortId) return null;
	const id = createWorkflowConnectionId(
		`connection:${sourceNodeId}:${sourcePortId}:${targetNodeId}:${targetPortId}`
	);
	if (!id) return null;
	return {
		id,
		source: { nodeId: sourceNodeId, portId: sourcePortId },
		target: { nodeId: targetNodeId, portId: targetPortId }
	};
}

function validateCandidate(
	connection: import('../domain/types').WorkflowConnection,
	workflow: WorkflowDefinition,
	registry: WorkflowNodeRegistry
): import('../domain/types').WorkflowValidationResult {
	const candidateWorkflow = workflowForCandidate(connection, workflow, registry);
	return validateWorkflow(candidateWorkflow, registry);
}

function workflowForCandidate(
	connection: import('../domain/types').WorkflowConnection,
	workflow: WorkflowDefinition,
	registry: WorkflowNodeRegistry
): WorkflowDefinition {
	const targetNode = workflow.nodes.find((node) => node.id === connection.target.nodeId);
	const targetDefinition = targetNode
		? registry.get(String(targetNode.kind), targetNode.definitionVersion)
		: undefined;
	const targetPort = targetDefinition?.inputs.find(
		(port) => String(port.id) === String(connection.target.portId)
	);
	const connections =
		targetPort?.cardinality === 'many'
			? workflow.connections
			: workflow.connections.filter(
					(value) =>
						value.target.nodeId !== connection.target.nodeId ||
						value.target.portId !== connection.target.portId
				);
	return {
		...workflow,
		connections: [...connections.filter((value) => value.id !== connection.id), connection]
	};
}

function sameReteConnection(
	current: WorkflowReteConnection,
	desired: import('../domain/types').WorkflowConnection
): boolean {
	return (
		current.source === desired.source.nodeId &&
		String(current.sourceOutput) === String(desired.source.portId) &&
		current.target === desired.target.nodeId &&
		String(current.targetInput) === String(desired.target.portId)
	);
}

function nodeNeedsRebuild(
	current: WorkflowReteNode,
	desired: WorkflowDefinition['nodes'][number],
	registry: WorkflowNodeRegistry
): boolean {
	if (
		current.workflowKind !== desired.kind ||
		current.workflowDefinitionVersion !== desired.definitionVersion
	) {
		return true;
	}
	const definition = registry.get(String(desired.kind), desired.definitionVersion);
	if (!definition && !current.workflowUnsupported) return true;
	if (definition && current.workflowUnsupported) return true;
	const inputKeys = Object.keys(current.inputs).sort();
	const outputKeys = Object.keys(current.outputs).sort();
	const desiredInputKeys = definition?.inputs.map((port) => String(port.id)).sort() ?? [];
	const desiredOutputKeys = definition?.outputs.map((port) => String(port.id)).sort() ?? [];
	if (!sameStrings(inputKeys, desiredInputKeys) || !sameStrings(outputKeys, desiredOutputKeys)) {
		return true;
	}
	const currentControlTypes = Object.entries(current.controls)
		.filter(
			(entry): entry is [string, WorkflowConfigControl] =>
				entry[1] instanceof WorkflowConfigControl
		)
		.map(([key, control]) => `${key}:${control.workflowConfigType}`)
		.sort();
	const desiredControlTypes = Object.entries(desired.config)
		.flatMap(([key, value]) =>
			isControlValue(value) ? [`${key}:${controlTypeForValue(value)}`] : []
		)
		.sort();
	return !sameStrings(currentControlTypes, desiredControlTypes);
}

function updateReteNode(
	current: WorkflowReteNode,
	desired: WorkflowDefinition['nodes'][number],
	registry: WorkflowNodeRegistry
): boolean {
	const changed = !sameWorkflowNode(current.workflowNode, desired);
	current.workflowNode = desired;
	const definition = registry.get(String(desired.kind), desired.definitionVersion);
	current.workflowTitle = definition?.title ?? String(desired.kind);
	current.workflowDescription = definition?.description;
	current.workflowCategory = definition?.category ?? 'Unsupported';
	current.workflowUnsupported = definition === undefined;
	current.label = current.workflowTitle;
	for (const [key, control] of Object.entries(current.controls)) {
		if (!(control instanceof WorkflowConfigControl)) continue;
		const value = desired.config[key];
		if (isControlValue(value) && control.value !== value) control.value = value;
	}
	return changed;
}

function sameWorkflowNode(
	left: WorkflowDefinition['nodes'][number],
	right: WorkflowDefinition['nodes'][number]
): boolean {
	return (
		left.id === right.id &&
		left.kind === right.kind &&
		left.definitionVersion === right.definitionVersion &&
		left.label === right.label &&
		JSON.stringify(left.config) === JSON.stringify(right.config) &&
		JSON.stringify(left.metadata) === JSON.stringify(right.metadata)
	);
}

function controlTypeForValue(value: string | number | boolean): 'text' | 'number' | 'boolean' {
	if (typeof value === 'string') return 'text';
	if (typeof value === 'number') return 'number';
	return 'boolean';
}

function sameStrings(left: readonly string[], right: readonly string[]): boolean {
	return left.length === right.length && left.every((value, index) => value === right[index]);
}

function sameWorkflowNodeIds(
	left: readonly WorkflowNodeId[],
	right: readonly WorkflowNodeId[]
): boolean {
	return left.length === right.length && left.every((value, index) => value === right[index]);
}

function clamp(value: number, minimum: number, maximum: number): number {
	return Math.min(maximum, Math.max(minimum, value));
}
