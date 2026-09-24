<script lang="ts">
	import { onMount } from 'svelte';
	import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
	import BotIcon from '@lucide/svelte/icons/bot';
	import CheckIcon from '@lucide/svelte/icons/check';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import GitBranchIcon from '@lucide/svelte/icons/git-branch';
	import KeyboardIcon from '@lucide/svelte/icons/keyboard';
	import Link2Icon from '@lucide/svelte/icons/link-2';
	import PlayIcon from '@lucide/svelte/icons/play';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import Redo2Icon from '@lucide/svelte/icons/redo-2';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SaveIcon from '@lucide/svelte/icons/save';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import SparklesIcon from '@lucide/svelte/icons/sparkles';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import Undo2Icon from '@lucide/svelte/icons/undo-2';
	import XIcon from '@lucide/svelte/icons/x';
	import ZapIcon from '@lucide/svelte/icons/zap';

	import type { AutomationDefinitionDto, AutomationRunDto } from '$lib/api/generated/models';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import WorkflowEditor from '$lib/features/workflows/editor/WorkflowEditor.svelte';
	import {
		AUTOMATION_STATUSES,
		AUTOMATION_TRIGGERS,
		DEFAULT_AUTOMATION_DRAFT,
		isAutomationStatus,
		isAutomationTrigger,
		labelForStatus,
		labelForTrigger,
		type AutomationDraft
	} from '../helpers';
	import {
		AUTOMATION_GRAPH_PALETTE,
		automationWorkflowStorageKey,
		cloneWorkflow,
		createHistory,
		createWorkflowNodeFromPalette,
		loadAutomationWorkflow,
		saveAutomationWorkflowDraft,
		workflowFingerprint,
		type AutomationWorkflowLoadResult
	} from './automation-workflow';
	import {
		workflowCommands,
		type WorkflowCommand,
		type WorkflowHistory
	} from '$lib/features/workflows/domain/commands';
	import {
		DEFAULT_WORKFLOW_REGISTRY,
		definitionPorts,
		type WorkflowNodeRegistry
	} from '$lib/features/workflows/domain/registry';
	import {
		createWorkflowNodeId,
		createWorkflowPortId,
		isJsonValue,
		workflowConnectionId,
		workflowNodeId,
		workflowNodeKind,
		type JsonValue,
		type WorkflowConnection,
		type WorkflowDefinition,
		type WorkflowNode,
		type WorkflowNodeId,
		type WorkflowNodeKind
	} from '$lib/features/workflows/domain/types';
	import { validateConnection } from '$lib/features/workflows/domain/validation';
	import type {
		WorkflowEditorChange,
		WorkflowEditorError,
		WorkflowEditorHandle
	} from '$lib/features/workflows/editor/rete-adapter';
	import AutomationRunHistory from './AutomationRunHistory.svelte';

	type AutomationEditorProps = {
		projectId: string;
		draft: AutomationDraft;
		mode: 'add' | 'edit';
		definition?: AutomationDefinitionDto;
		isSaving: boolean;
		isDirty: boolean;
		nameIsValid: boolean;
		nameAlreadyUsed: boolean;
		configurationError: string | null;
		feedback: string | null;
		manualReady: boolean;
		manualPending: boolean;
		manualError: string | null;
		runs: AutomationRunDto[];
		selectedRun?: AutomationRunDto;
		selectedRunLoading: boolean;
		selectedRunError: string | null;
		onDraftChange: (draft: AutomationDraft) => void;
		onDirtyChange: (dirty: boolean) => void;
		onSave: () => Promise<boolean>;
		onClose: () => void;
		onRunManually: () => void | Promise<void>;
		onRefresh: () => void | Promise<void>;
		onSelectRun: (runId: string) => void;
		onReturnFocus?: () => void;
	};

	type PortOption = {
		value: string;
		label: string;
		nodeId: string;
		portId: string;
		dataType: string;
	};

	let {
		projectId,
		draft,
		mode,
		definition,
		isSaving,
		isDirty,
		nameIsValid,
		nameAlreadyUsed,
		configurationError,
		feedback,
		manualReady,
		manualPending,
		manualError,
		runs,
		selectedRun,
		selectedRunLoading,
		selectedRunError,
		onDraftChange,
		onDirtyChange,
		onSave,
		onClose,
		onRunManually,
		onRefresh,
		onSelectRun,
		onReturnFocus
	}: AutomationEditorProps = $props();

	const registry: WorkflowNodeRegistry = DEFAULT_WORKFLOW_REGISTRY;
	const isEditing = $derived(mode === 'edit');
	const saving = $derived(isSaving);

	let editorRoot = $state<HTMLDialogElement | null>(null);
	let returnFocusTarget: HTMLElement | null = null;
	let editorController = $state<WorkflowEditorHandle | null>(null);
	let workingDraft = $state<AutomationDraft>({ ...DEFAULT_AUTOMATION_DRAFT });
	// Workflow documents are replaced wholesale by the command history and are
	// handed to the Rete adapter, whose initialization uses structuredClone.
	// Keep these immutable document snapshots raw so Svelte does not wrap them
	// in a Proxy that structuredClone cannot accept.
	let workflow = $state.raw<WorkflowDefinition | null>(null);
	let workflowLoad = $state.raw<AutomationWorkflowLoadResult | null>(null);
	let history = $state<WorkflowHistory | null>(null);
	let selectedNodeId = $state<WorkflowNodeId | null>(null);
	let selectedConnectionId = $state<string | null>(null);
	let requestedNodeSelection = $state<WorkflowNodeId | null>(null);
	let nodeSequence = $state(0);
	let workflowRevision = $state(0);
	let graphDirty = $state(false);
	let graphLoadedForKey = $state<string | null>(null);
	let graphRestored = $state(false);
	let localStorageState = $state<'unknown' | 'available' | 'unavailable'>('unknown');
	let editorNotice = $state<string | null>(null);
	let editorError = $state<string | null>(null);
	let lastDraftFingerprint = $state('');
	let sourceNodeId = $state<string>('');
	let sourcePortId = $state<string>('');
	let targetNodeId = $state<string>('');
	let targetPortId = $state<string>('');

	const draftFingerprint = $derived(`${draft.name}\u0000${draft.trigger}\u0000${draft.status}`);
	const storageIdentity = $derived(definition?.id ?? 'new');
	const storageKey = $derived(automationWorkflowStorageKey(projectId, storageIdentity));
	const selectedNode = $derived(
		workflow?.nodes.find((node) => node.id === selectedNodeId) ?? null
	);
	const selectedConnection = $derived(
		workflow?.connections.find(
			(connection) => String(connection.id) === selectedConnectionId
		) ?? null
	);
	const selectedConfigEntries = $derived.by(() =>
		selectedNode ? Object.entries(selectedNode.config) : []
	);
	const localNameIsValid = $derived(
		workingDraft.name.trim().length > 0 && workingDraft.name.trim().length <= 200
	);
	const validName = $derived(nameIsValid && localNameIsValid && !nameAlreadyUsed);
	const graphHasUnsavedChanges = $derived(graphDirty);
	const historyState = $derived.by(() => {
		const revision = workflowRevision;
		return {
			revision,
			canUndo: history?.canUndo ?? false,
			canRedo: history?.canRedo ?? false
		};
	});
	const errorMessages = $derived(
		[configurationError, manualError, selectedRunError, editorError].filter(
			(message): message is string => Boolean(message)
		)
	);
	const graphStatusLabel = $derived(
		graphHasUnsavedChanges ? 'Unsaved graph draft' : 'Graph draft saved'
	);
	const localStorageLabel = $derived(
		localStorageState === 'unavailable'
			? 'Local drafts unavailable'
			: graphRestored
				? 'Restored from this browser'
				: 'Local draft storage ready'
	);
	const graphNodes = $derived(workflow?.nodes ?? []);
	const graphConnections = $derived(workflow?.connections ?? []);
	const sourceOptions = $derived.by(() => portOptions('output'));
	const targetOptions = $derived.by(() => portOptions('input'));

	$effect(() => {
		if (draftFingerprint === lastDraftFingerprint) return;
		workingDraft = { ...draft };
		lastDraftFingerprint = draftFingerprint;
	});

	onMount(() => {
		returnFocusTarget =
			document.activeElement instanceof HTMLElement ? document.activeElement : null;
		const dialog = editorRoot;
		if (!dialog) return;
		if (!dialog.open) dialog.showModal();
		dialog.focus({ preventScroll: true });

		return () => {
			if (dialog.open) dialog.close();
			if (onReturnFocus) {
				onReturnFocus();
				return;
			}
			const target = returnFocusTarget;
			if (!target) return;
			const restore = () => {
				if (target.isConnected) target.focus({ preventScroll: true });
			};
			if (typeof window.requestAnimationFrame === 'function') {
				window.requestAnimationFrame(restore);
			} else {
				window.setTimeout(restore, 0);
			}
		};
	});

	$effect(() => {
		const key = storageKey;
		if (key === graphLoadedForKey) return;
		graphLoadedForKey = key;
		const result = loadAutomationWorkflow({
			key,
			draft,
			projectId,
			identity: storageIdentity,
			name: workingDraft.name
		});
		workflowLoad = result;
		workflow = cloneWorkflow(result.workflow);
		history = createHistory(result.workflow, registry);
		workflowRevision += 1;
		selectedNodeId = null;
		selectedConnectionId = null;
		graphDirty = false;
		graphRestored = result.restored;
		localStorageState = result.available ? 'available' : 'unavailable';
		editorError = null;
		editorNotice = result.error;
		if (result.warnings.length > 0) {
			editorNotice = `${result.warnings.join(' ')} Review the restored draft before saving.`;
		}
	});

	// Palette actions update the canonical document before the Rete view has
	// reconciled its transient node collection. Queue the selection after that
	// reconciliation so the newly-created node remains selected for keyboard
	// and inspector actions.
	$effect(() => {
		const controller = editorController;
		const nextWorkflow = workflow;
		const requested = requestedNodeSelection;
		if (!controller || !nextWorkflow || !requested) return;
		void controller
			.reconcile(nextWorkflow)
			.then(() => controller.selectNode(requested))
			.then(() => {
				if (requestedNodeSelection === requested) requestedNodeSelection = null;
			})
			.catch((error: unknown) => {
				if (requestedNodeSelection === requested) requestedNodeSelection = null;
				handleControllerError(error);
			});
	});

	function readControlValue(event: Event): string | undefined {
		const target = event.currentTarget;
		return target instanceof HTMLInputElement ||
			target instanceof HTMLTextAreaElement ||
			target instanceof HTMLSelectElement
			? target.value
			: undefined;
	}

	function updateDraftName(event: Event): void {
		if (isEditing) return;
		const value = readControlValue(event);
		if (value === undefined) return;
		const nextDraft = { ...workingDraft, name: value } satisfies AutomationDraft;
		workingDraft = nextDraft;
		onDraftChange(nextDraft);
	}

	function updateDraftTrigger(event: Event): void {
		const value = readControlValue(event);
		if (!isAutomationTrigger(value)) return;
		const nextDraft = { ...workingDraft, trigger: value } satisfies AutomationDraft;
		workingDraft = nextDraft;
		onDraftChange(nextDraft);
		updateTriggerConfig(value, workingDraft.status);
	}

	function updateDraftStatus(event: Event): void {
		const value = readControlValue(event);
		if (!isAutomationStatus(value)) return;
		const nextDraft = { ...workingDraft, status: value } satisfies AutomationDraft;
		workingDraft = nextDraft;
		onDraftChange(nextDraft);
		updateTriggerConfig(workingDraft.trigger, value);
	}

	function updateTriggerConfig(
		trigger: AutomationDraft['trigger'],
		status: AutomationDraft['status']
	): void {
		if (!workflow) return;
		const triggerNode = workflow.nodes.find((node) => String(node.kind) === 'event_trigger');
		if (!triggerNode) return;
		commitCommand(
			workflowCommands.updateNodeConfig(triggerNode.id, {
				trigger,
				status
			})
		);
	}

	function commitCommand(command: WorkflowCommand): boolean {
		const currentHistory = history;
		if (!currentHistory) return false;
		const result = currentHistory.execute(command);
		if (!result.ok) {
			editorError = result.issues.map((issue) => issue.message).join(' ');
			return false;
		}
		workflow = result.workflow;
		workflowRevision += 1;
		graphDirty = true;
		editorNotice = null;
		editorError = null;
		onDirtyChange(true);
		return true;
	}

	function handleWorkflowChange(change: WorkflowEditorChange): void {
		if (!workflow || workflowFingerprint(change.workflow) === workflowFingerprint(workflow))
			return;
		if (change.type === 'node-removed' && change.nodeId === selectedNodeId)
			selectedNodeId = null;
		if (change.type === 'connection-removed' && change.connectionId === selectedConnectionId)
			selectedConnectionId = null;
		const commands = commandsForEditorChange(workflow, change.workflow, change);
		for (const command of commands) {
			if (!commitCommand(command)) return;
		}
	}

	function commandsForEditorChange(
		current: WorkflowDefinition,
		next: WorkflowDefinition,
		change: WorkflowEditorChange
	): WorkflowCommand[] {
		const commands: WorkflowCommand[] = [];
		switch (change.type) {
			case 'node-added':
				for (const node of next.nodes) {
					if (current.nodes.some((candidate) => candidate.id === node.id)) continue;
					commands.push(
						workflowCommands.addNode(node, next.layout?.nodes[String(node.id)])
					);
				}
				break;
			case 'node-removed':
				for (const node of current.nodes) {
					if (!next.nodes.some((candidate) => candidate.id === node.id)) {
						commands.push(workflowCommands.removeNode(node.id));
					}
				}
				break;
			case 'node-moved':
				for (const node of next.nodes) {
					const previous = current.layout?.nodes[String(node.id)];
					const position = next.layout?.nodes[String(node.id)];
					if (!position || samePosition(previous, position)) continue;
					commands.push(workflowCommands.updateNodePosition(node.id, position));
				}
				break;
			case 'node-configured':
				for (const node of next.nodes) {
					const previous = current.nodes.find((candidate) => candidate.id === node.id);
					if (!previous) continue;
					const changedConfig = changedJsonEntries(previous.config, node.config);
					if (Object.keys(changedConfig).length > 0) {
						commands.push(workflowCommands.updateNodeConfig(node.id, changedConfig));
					}
				}
				break;
			case 'connection-added':
				for (const connection of current.connections) {
					if (!next.connections.some((candidate) => candidate.id === connection.id)) {
						commands.push(workflowCommands.removeConnection(String(connection.id)));
					}
				}
				for (const connection of next.connections) {
					if (!current.connections.some((candidate) => candidate.id === connection.id)) {
						commands.push(workflowCommands.addConnection(connection));
					}
				}
				break;
			case 'connection-removed':
				for (const connection of current.connections) {
					if (!next.connections.some((candidate) => candidate.id === connection.id)) {
						commands.push(workflowCommands.removeConnection(String(connection.id)));
					}
				}
				break;
			default: {
				const exhaustive: never = change.type;
				return exhaustive;
			}
		}
		return commands;
	}

	function samePosition(
		left: { x: number; y: number } | undefined,
		right: { x: number; y: number }
	): boolean {
		return left?.x === right.x && left?.y === right.y;
	}

	function changedJsonEntries(
		previous: Record<string, JsonValue>,
		next: Record<string, JsonValue>
	): Record<string, JsonValue> {
		const changed: Record<string, JsonValue> = {};
		for (const [key, value] of Object.entries(next)) {
			if (JSON.stringify(previous[key]) !== JSON.stringify(value)) changed[key] = value;
		}
		return changed;
	}

	function handleWorkflowSelection(nodeIds: readonly WorkflowNodeId[]): void {
		if (nodeIds.length === 0 && requestedNodeSelection !== null) return;
		selectedNodeId = nodeIds[0] ?? null;
		selectedConnectionId = null;
		if (nodeIds.length > 0) requestedNodeSelection = null;
	}

	function handleWorkflowError(error: WorkflowEditorError): void {
		editorError = error.message;
	}

	function handleControllerError(error: unknown): void {
		editorError =
			error instanceof Error ? error.message : 'The workflow editor could not update.';
	}

	function createNodeId(kind: WorkflowNodeKind): string {
		nodeSequence += 1;
		const random =
			typeof globalThis.crypto?.randomUUID === 'function'
				? globalThis.crypto.randomUUID().replaceAll('-', '').slice(0, 10)
				: `${Date.now()}${nodeSequence}`;
		return `${String(kind).replace(/[^A-Za-z0-9._:-]/g, '-')}-${random}`;
	}

	function nextNodePosition(index: number): { x: number; y: number } {
		const column = index % 3;
		const row = Math.floor(index / 3);
		return { x: 80 + column * 320, y: 100 + row * 210 };
	}

	function addPaletteNode(item: (typeof AUTOMATION_GRAPH_PALETTE)[number]): void {
		if (!workflow || !editable()) return;
		const position = nextNodePosition(workflow.nodes.length);
		const node = createWorkflowNodeFromPalette({
			item,
			id: createNodeId(paletteCanonicalKind(item)),
			position,
			draft: workingDraft
		});
		if (commitCommand(workflowCommands.addNode(node, position))) {
			selectedNodeId = node.id;
			selectedConnectionId = null;
			requestedNodeSelection = node.id;
		}
	}

	function duplicateSelectedNode(): void {
		if (!workflow || !selectedNode || !editable()) return;
		const position = {
			x: workflow.layout?.nodes[String(selectedNode.id)]?.x ?? 80,
			y: workflow.layout?.nodes[String(selectedNode.id)]?.y ?? 100
		};
		const duplicate = {
			...cloneNode(selectedNode),
			id: workflowNodeId(createNodeId(selectedNode.kind)),
			metadata: { ...(selectedNode.metadata ?? {}) }
		};
		const nextPosition = { x: position.x + 48, y: position.y + 48 };
		if (commitCommand(workflowCommands.addNode(duplicate, nextPosition))) {
			selectedNodeId = duplicate.id;
			selectedConnectionId = null;
			requestedNodeSelection = duplicate.id;
		}
	}

	function removeSelection(): void {
		if (!workflow || !editable()) return;
		if (selectedNodeId) {
			if (commitCommand(workflowCommands.removeNode(selectedNodeId))) selectedNodeId = null;
			return;
		}
		if (selectedConnectionId) {
			if (commitCommand(workflowCommands.removeConnection(selectedConnectionId))) {
				selectedConnectionId = null;
			}
		}
	}

	function tidyGraph(): void {
		if (!workflow || !editable()) return;
		for (const [index, node] of workflow.nodes.entries()) {
			const position = nextNodePosition(index);
			const previous = workflow.layout?.nodes[String(node.id)];
			if (samePosition(previous, position)) continue;
			if (!commitCommand(workflowCommands.updateNodePosition(node.id, position))) return;
		}
		void editorController?.fitView().catch(handleControllerError);
	}

	function updateNodeConfig(key: string, event: Event): void {
		if (!workflow || !selectedNode || !editable()) return;
		const raw = readControlValue(event);
		if (raw === undefined) return;
		const previous = selectedNode.config[key];
		const value = parseConfigValue(raw, previous);
		if (value === null && previous !== null) {
			editorError = `The ${key} configuration value is not valid.`;
			return;
		}
		commitCommand(workflowCommands.updateNodeConfig(selectedNode.id, { [key]: value }));
	}

	function updateNodeLayout(nodeId: string, event: Event): void {
		const raw = readControlValue(event);
		if (!raw || !workflow || !editable()) return;
		const [xRaw, yRaw] = raw.split(',');
		const x = Number(xRaw);
		const y = Number(yRaw);
		if (!Number.isFinite(x) || !Number.isFinite(y)) return;
		const parsedNodeId = createWorkflowNodeId(nodeId);
		if (!parsedNodeId) return;
		commitCommand(workflowCommands.updateNodePosition(parsedNodeId, { x, y }));
	}

	function saveGraphDraft(): void {
		if (!workflow) return;
		if (workflowLoad?.protectedDraft) {
			editorNotice =
				'This draft is protected because it could not be read safely. Resolve the draft or use a new automation before saving over it.';
			return;
		}
		if (!saveAutomationWorkflowDraft(storageKey, workflow)) {
			localStorageState = 'unavailable';
			editorNotice =
				'This browser blocked local storage. Server recipe settings can still be saved.';
			return;
		}
		localStorageState = 'available';
		graphRestored = true;
		graphDirty = false;
		onDirtyChange(false);
		editorNotice = 'Graph draft saved in this browser only.';
	}

	async function saveSettings(): Promise<void> {
		if (saving || !validName) return;
		try {
			await onSave();
		} catch {
			// The parent owns the server error state.
		}
	}

	function runBuiltInRecipe(): void {
		if (!manualReady || manualPending) return;
		void Promise.resolve(onRunManually()).catch(() => undefined);
	}

	function undo(): void {
		if (!editable()) return;
		const result = history?.undo();
		if (!result || !result.ok) {
			if (result && !result.ok)
				editorError = result.issues.map((issue) => issue.message).join(' ');
			return;
		}
		workflow = result.workflow;
		workflowRevision += 1;
		graphDirty = true;
		onDirtyChange(true);
		editorNotice = null;
	}

	function redo(): void {
		if (!editable()) return;
		const previousNodes = workflow?.nodes ?? [];
		const result = history?.redo();
		if (!result || !result.ok) {
			if (result && !result.ok)
				editorError = result.issues.map((issue) => issue.message).join(' ');
			return;
		}
		const addedNode = result.workflow.nodes.find(
			(node) => !previousNodes.some((prev) => prev.id === node.id)
		);
		workflow = result.workflow;
		if (addedNode) selectedNodeId = addedNode.id;
		workflowRevision += 1;
		graphDirty = true;
		onDirtyChange(true);
		editorNotice = null;
	}

	function handleWindowKeydown(event: KeyboardEvent): void {
		if (event.defaultPrevented) return;
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'z') {
			if (!editable() || isFormTarget(event.target)) return;
			event.preventDefault();
			if (event.shiftKey) redo();
			else undo();
			return;
		}
		if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'y') {
			if (!editable() || isFormTarget(event.target)) return;
			event.preventDefault();
			redo();
			return;
		}
		if (event.key === 'Delete' || event.key === 'Backspace') {
			if (!editable() || isFormTarget(event.target)) return;
			if (selectedNodeId || selectedConnectionId) {
				event.preventDefault();
				removeSelection();
			}
		}
	}

	function handleDialogCancel(event: Event): void {
		event.preventDefault();
		onClose();
	}

	function connectFromForm(): void {
		if (!workflow || !editable()) return;
		if (!sourceNodeId || !sourcePortId || !targetNodeId || !targetPortId) {
			editorError = 'Choose an output and an input before connecting them.';
			return;
		}
		const source = createWorkflowNodeId(sourceNodeId);
		const target = createWorkflowNodeId(targetNodeId);
		const sourcePort = createWorkflowPortId(sourcePortId);
		const targetPort = createWorkflowPortId(targetPortId);
		if (!source || !target || !sourcePort || !targetPort) {
			editorError = 'Connection endpoints are invalid.';
			return;
		}
		const candidate: WorkflowConnection = {
			id: connectionId(),
			source: { nodeId: source, portId: sourcePort },
			target: { nodeId: target, portId: targetPort }
		};
		const validation = validateConnection(candidate, workflow, registry);
		if (!validation.valid) {
			editorError = validation.issues.map((issue) => issue.message).join(' ');
			return;
		}
		if (!commitCommand(workflowCommands.addConnection(candidate))) return;
		editorError = null;
		selectedConnectionId = String(candidate.id);
		selectedNodeId = null;
	}

	function connectionId(): ReturnType<typeof workflowConnectionId> {
		let candidate = `connection-${Date.now()}-${nodeSequence}`;
		while (workflow?.connections.some((connection) => String(connection.id) === candidate)) {
			nodeSequence += 1;
			candidate = `connection-${Date.now()}-${nodeSequence}`;
		}
		return workflowConnectionId(candidate);
	}

	function portOptions(direction: 'input' | 'output'): PortOption[] {
		if (!workflow) return [];
		return workflow.nodes.flatMap((node) => {
			const definition = registry.get(String(node.kind), node.definitionVersion);
			if (!definition) return [];
			return definitionPorts(definition, direction).map((port) => ({
				value: `${String(node.id)}::${String(port.id)}`,
				label: `${nodeTitle(node)} · ${port.label ?? String(port.id)} · ${String(port.dataType)}`,
				nodeId: String(node.id),
				portId: String(port.id),
				dataType: String(port.dataType)
			}));
		});
	}

	function updateSource(value: string): void {
		const [nodeId, portId] = value.split('::');
		sourceNodeId = nodeId ?? '';
		sourcePortId = portId ?? '';
	}

	function updateTarget(value: string): void {
		const [nodeId, portId] = value.split('::');
		targetNodeId = nodeId ?? '';
		targetPortId = portId ?? '';
	}

	function handleAccessibleNodeSelect(nodeId: WorkflowNodeId): void {
		selectedNodeId = nodeId;
		selectedConnectionId = null;
		requestedNodeSelection = null;
		void editorController?.selectNode(nodeId).catch(handleControllerError);
	}

	function selectedLayout(node: WorkflowNode): string {
		const layout = workflow?.layout?.nodes[String(node.id)];
		return layout ? `${layout.x},${layout.y}` : '';
	}

	function nodeTitle(node: WorkflowNode): string {
		return registry.get(String(node.kind), node.definitionVersion)?.title ?? String(node.kind);
	}

	function paletteCanonicalKind(
		item: (typeof AUTOMATION_GRAPH_PALETTE)[number]
	): WorkflowNodeKind {
		const itemKey: string = item.key;
		switch (item.kind) {
			case 'trigger':
				return workflowNodeKind('event_trigger');
			case 'action':
				return workflowNodeKind(
					itemKey === 'recompute_project_metrics' ? itemKey : 'deterministic_action'
				);
			case 'condition':
				return workflowNodeKind('condition');
			case 'agent':
				return workflowNodeKind('agent_step');
			case 'human_gate':
				return workflowNodeKind('human_gate');
			default: {
				const exhaustive: never = item;
				return exhaustive;
			}
		}
	}

	function parseConfigValue(raw: string, previous: JsonValue | undefined): JsonValue {
		if (typeof previous === 'number') {
			const number = Number(raw);
			return Number.isFinite(number) ? number : null;
		}
		if (typeof previous === 'boolean') return raw === 'true';
		return raw;
	}

	function editable(): boolean {
		return workflow !== null && !saving;
	}

	function cloneNode(node: WorkflowNode): WorkflowNode {
		return {
			...node,
			config: { ...node.config },
			metadata: node.metadata ? { ...node.metadata } : undefined
		};
	}

	function iconForKind(kind: string): typeof ZapIcon {
		switch (kind) {
			case 'trigger':
				return ZapIcon;
			case 'action':
				return PlayIcon;
			case 'condition':
				return GitBranchIcon;
			case 'agent':
				return BotIcon;
			case 'human_gate':
				return ShieldCheckIcon;
			default:
				return SparklesIcon;
		}
	}

	function isFormTarget(target: EventTarget | null): boolean {
		return (
			target instanceof Element &&
			Boolean(target.closest('input, select, textarea, [contenteditable="true"]'))
		);
	}
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<dialog
	bind:this={editorRoot}
	class="automation-editor"
	aria-modal="true"
	aria-labelledby="automation-editor-title"
	aria-describedby="automation-editor-description"
	tabindex="-1"
	oncancel={handleDialogCancel}
	data-testid="automation-editor"
	data-automation-graph-state={graphHasUnsavedChanges ? 'unsaved' : 'saved'}
	data-automation-settings-state={isDirty ? 'unsaved' : 'saved'}
>
	<header class="editor-header">
		<div class="editor-heading">
			<Button
				variant="ghost"
				size="icon-sm"
				data-testid="automation-editor-back"
				aria-label="Back to automations"
				onclick={onClose}><ArrowLeftIcon /></Button
			>
			<div class="editor-title-copy">
				<span class="editor-eyebrow">Automation editor</span>
				<h1 id="automation-editor-title">
					{workingDraft.name.trim() || 'Untitled automation'}
				</h1>
			</div>
		</div>
		<div class="editor-actions">
			<span class="graph-status" data-testid="automation-editor-local-status"
				><span
					class={['status-dot', graphHasUnsavedChanges && 'status-dirty']}
					aria-hidden="true"
				></span>{graphStatusLabel}</span
			>
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label="Refresh automation activity"
				onclick={() => void onRefresh()}><RefreshCwIcon /></Button
			>
			<Button
				variant="ghost"
				size="icon-sm"
				data-testid="automation-editor-undo"
				aria-label="Undo graph change"
				disabled={!historyState.canUndo || !editable()}
				onclick={undo}><Undo2Icon /></Button
			>
			<Button
				variant="ghost"
				size="icon-sm"
				data-testid="automation-editor-redo"
				aria-label="Redo graph change"
				disabled={!historyState.canRedo || !editable()}
				onclick={redo}><Redo2Icon /></Button
			>
			<Button
				variant="outline"
				size="sm"
				data-testid="automation-editor-save-graph"
				disabled={workflowLoad?.protectedDraft === true}
				onclick={saveGraphDraft}
				><SaveIcon data-icon="inline-start" /> Save graph draft</Button
			>
			<Button
				variant="outline"
				size="sm"
				data-testid="automation-editor-run"
				disabled={!manualReady || manualPending}
				onclick={runBuiltInRecipe}
				><PlayIcon data-icon="inline-start" />
				{manualPending ? 'Starting…' : 'Run built-in recipe'}</Button
			>
			<Button
				data-testid="automation-editor-save-settings"
				disabled={!validName || saving}
				onclick={saveSettings}
				><SaveIcon data-icon="inline-start" />
				{saving ? 'Saving…' : 'Save settings'}</Button
			>
		</div>
	</header>

	<div
		id="automation-editor-description"
		class="editor-boundary"
		data-testid="automation-editor-boundary"
	>
		<div class="boundary-mark" aria-hidden="true"><KeyboardIcon /></div>
		<div>
			<strong>Graph draft · local only</strong>
			<p>
				Custom graphs are saved in this browser. Running an automation uses its saved
				trigger and status with the built-in maintenance step.
			</p>
		</div>
		<Badge variant="outline">{localStorageLabel}</Badge>
	</div>
	{#if errorMessages.length > 0}<div
			class="editor-errors"
			role="alert"
			data-testid="automation-editor-errors"
		>
			<XIcon aria-hidden="true" />
			<div>
				<strong>Automation editor needs attention</strong
				>{#each errorMessages as message (message)}<p>{message}</p>{/each}
			</div>
		</div>{/if}
	{#if feedback}<div
			class="editor-feedback"
			role="status"
			data-testid="automation-editor-feedback"
		>
			<CheckIcon aria-hidden="true" /><span>{feedback}</span>
		</div>{/if}
	{#if editorNotice}<div
			class="editor-notice"
			role="status"
			data-testid="automation-editor-notice"
		>
			<XIcon aria-hidden="true" /><span>{editorNotice}</span>
		</div>{/if}

	<div class="editor-body">
		<aside
			class="editor-palette"
			aria-label="Automation node palette"
			data-testid="automation-editor-palette"
		>
			<div class="panel-heading">
				<div>
					<span class="panel-kicker">Build the draft</span>
					<h2>Node palette</h2>
				</div>
				<Badge variant="outline">{graphNodes.length} nodes</Badge>
			</div>
			<p class="panel-description">
				Add a typed visual step to the local draft. These nodes describe intent and are not
				sent as executable steps by the current API.
			</p>
			<div class="palette-list">
				{#each AUTOMATION_GRAPH_PALETTE as item (item.kind)}{@const Icon = iconForKind(
						item.kind
					)}<button
						type="button"
						class="palette-item"
						onclick={() => addPaletteNode(item)}
						data-testid={`automation-editor-add-${item.kind}`}
						><span class="palette-icon" data-kind={item.kind} aria-hidden="true"
							><Icon /></span
						><span class="palette-copy"
							><strong>{item.label}</strong><small>{item.description}</small></span
						><span class="palette-add" aria-hidden="true"><PlusIcon /></span></button
					>{/each}
			</div>
			<div class="palette-footer">
				<SparklesIcon aria-hidden="true" /><span
					>Use the node list or canvas to select a step. Keyboard shortcuts: ⌘/Ctrl+Z and
					Delete.</span
				>
			</div>
		</aside>

		<section
			class="editor-canvas"
			aria-label="Automation graph canvas"
			data-testid="automation-editor-canvas"
		>
			<div class="canvas-toolbar">
				<div>
					<span class="canvas-label">Typed visual draft</span><span class="canvas-meta"
						>{graphNodes.length} nodes · {graphConnections.length} connections</span
					>
				</div>
				<div class="canvas-toolbar-actions">
					<Button variant="ghost" size="sm" onclick={tidyGraph}
						><SparklesIcon data-icon="inline-start" /> Tidy layout</Button
					><Button
						variant="ghost"
						size="icon-sm"
						disabled={!selectedNode && !selectedConnection}
						aria-label="Delete selected graph item"
						onclick={removeSelection}><Trash2Icon /></Button
					>
				</div>
			</div>
			<div class="flow-frame">
				{#if workflow}<WorkflowEditor
						bind:controller={editorController}
						{workflow}
						{registry}
						readOnly={false}
						onChange={handleWorkflowChange}
						onSelectionChange={handleWorkflowSelection}
						onError={handleWorkflowError}
					/>{:else}<div class="editor-loading" role="status">
						Loading workflow canvas…
					</div>{/if}
			</div>
		</section>

		<aside
			class="editor-inspector"
			aria-label="Automation inspector"
			data-testid="automation-editor-inspector"
		>
			<div class="panel-heading">
				<div>
					<span class="panel-kicker">Configuration</span>
					<h2>Inspector</h2>
				</div>
				{#if selectedNode || selectedConnection}<Button
						variant="ghost"
						size="icon-xs"
						aria-label="Clear inspector selection"
						onclick={() => {
							selectedNodeId = null;
							selectedConnectionId = null;
						}}><XIcon /></Button
					>{/if}
			</div>
			<div class="inspector-scroll">
				<section class="inspector-section" aria-labelledby="definition-settings-heading">
					<div class="section-heading">
						<div>
							<span class="panel-kicker">Built-in recipe</span>
							<h3 id="definition-settings-heading">Definition settings</h3>
						</div>
						<Badge variant={isEditing ? 'secondary' : 'outline'}
							>{isEditing ? 'Editing' : 'New'}</Badge
						>
					</div>
					<label class="field-label" for="automation-editor-name">Name</label><input
						id="automation-editor-name"
						disabled={saving}
						class="field-control"
						value={workingDraft.name}
						readonly={isEditing}
						maxlength="200"
						aria-invalid={!validName}
						oninput={updateDraftName}
					/>{#if !validName}<span class="field-error"
							>{nameAlreadyUsed
								? 'An automation with this name already exists.'
								: 'Name is required and must be at most 200 characters.'}</span
						>{/if}<label class="field-label" for="automation-editor-trigger"
						>Trigger</label
					><select
						id="automation-editor-trigger"
						disabled={saving}
						class="field-control"
						value={workingDraft.trigger}
						onchange={updateDraftTrigger}
						>{#each AUTOMATION_TRIGGERS as trigger (trigger)}<option value={trigger}
								>{labelForTrigger(trigger)}</option
							>{/each}</select
					><label class="field-label" for="automation-editor-status">Status</label><select
						id="automation-editor-status"
						disabled={saving}
						class="field-control"
						value={workingDraft.status}
						onchange={updateDraftStatus}
						>{#each AUTOMATION_STATUSES as statusOption (statusOption)}<option
								value={statusOption}>{labelForStatus(statusOption)}</option
							>{/each}</select
					>
					<p class="field-help">
						These settings control the built-in maintenance automation. Save graph draft
						stores the typed canvas locally.
					</p>
				</section>

				<section class="inspector-section" aria-labelledby="accessible-nodes-heading">
					<div class="section-heading">
						<div>
							<span class="panel-kicker">Keyboard access</span>
							<h3 id="accessible-nodes-heading">Workflow nodes</h3>
						</div>
						<Badge variant="outline">{graphNodes.length}</Badge>
					</div>
					<ul class="accessible-node-list" aria-label="Workflow nodes">
						{#if graphNodes.length === 0}<li class="field-help">
								No nodes in this draft.
							</li>{/if}{#each graphNodes as node (node.id)}<li>
								<button
									type="button"
									class={[
										'accessible-node',
										selectedNodeId === node.id && 'selected'
									]}
									aria-label={`Select ${nodeTitle(node)}`}
									aria-pressed={selectedNodeId === node.id}
									onclick={() => handleAccessibleNodeSelect(node.id)}
									><span
										><strong>{nodeTitle(node)}</strong><small
											>{String(node.kind)}</small
										></span
									><span aria-hidden="true">›</span></button
								>
							</li>{/each}
					</ul>
				</section>

				{#if selectedNode}<section
						class="inspector-section"
						aria-labelledby="node-settings-heading"
					>
						<div class="section-heading">
							<div>
								<span class="panel-kicker">Selected node</span>
								<h3 id="node-settings-heading">{nodeTitle(selectedNode)}</h3>
							</div>
							<Badge variant="outline">{String(selectedNode.kind)}</Badge>
						</div>
						<p class="field-help">
							Stable operation kind and typed ports come from the workflow registry.
						</p>
						{#if selectedConfigEntries.length > 0}<div class="config-fields">
								<span class="field-label">Parameters</span
								>{#each selectedConfigEntries as [key, value] (key)}<label
										class="config-field"
										for={`automation-node-config-${key}`}
										><span>{key}</span
										>{#if isJsonValue(value) && (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean')}<input
												id={`automation-node-config-${key}`}
												class="field-control field-mono"
												type={typeof value === 'number' ? 'number' : 'text'}
												value={String(value)}
												oninput={(event) => updateNodeConfig(key, event)}
											/>{:else}<code class="config-value"
												>{JSON.stringify(value)}</code
											>{/if}</label
									>{/each}
							</div>{:else}<p class="field-help">
								This operation has no editable parameters.
							</p>{/if}{#if workflow?.layout?.nodes[String(selectedNode.id)]}<label
								class="field-label"
								for="automation-node-layout">Position</label
							><input
								id="automation-node-layout"
								class="field-control field-mono"
								value={selectedLayout(selectedNode)}
								oninput={(event) =>
									updateNodeLayout(String(selectedNode.id), event)}
							/>{/if}
						<div class="inspector-actions">
							<Button variant="outline" size="sm" onclick={duplicateSelectedNode}
								><CopyIcon data-icon="inline-start" />Duplicate</Button
							><Button variant="destructive" size="sm" onclick={removeSelection}
								><Trash2Icon data-icon="inline-start" />Delete</Button
							>
						</div>
					</section>{:else if selectedConnection}<section
						class="inspector-section"
						aria-labelledby="connection-settings-heading"
					>
						<div class="section-heading">
							<div>
								<span class="panel-kicker">Selected connection</span>
								<h3 id="connection-settings-heading">Typed connection</h3>
							</div>
							<Badge variant="outline"
								>{String(selectedConnection.source.portId)} → {String(
									selectedConnection.target.portId
								)}</Badge
							>
						</div>
						<p class="field-help">
							This connection follows the source and target port contracts in the
							workflow registry.
						</p>
						<Button variant="destructive" size="sm" onclick={removeSelection}
							><Trash2Icon data-icon="inline-start" />Delete connection</Button
						>
					</section>{/if}

				<section class="inspector-section" aria-labelledby="connection-form-heading">
					<div class="section-heading">
						<div>
							<span class="panel-kicker">Keyboard access</span>
							<h3 id="connection-form-heading">Connect typed ports</h3>
						</div>
						<Link2Icon aria-hidden="true" />
					</div>
					<p class="field-help">
						Choose an output and input to create a connection without dragging.
						Incompatible types are rejected with an explanation.
					</p>
					<label class="field-label" for="automation-connection-source">Output</label
					><select
						id="automation-connection-source"
						class="field-control"
						value={`${sourceNodeId}::${sourcePortId}`}
						onchange={(event) => updateSource(readControlValue(event) ?? '')}
						><option value="">Choose output</option
						>{#each sourceOptions as option (option.value)}<option value={option.value}
								>{option.label}</option
							>{/each}</select
					><label class="field-label" for="automation-connection-target">Input</label
					><select
						id="automation-connection-target"
						class="field-control"
						value={`${targetNodeId}::${targetPortId}`}
						onchange={(event) => updateTarget(readControlValue(event) ?? '')}
						><option value="">Choose input</option
						>{#each targetOptions as option (option.value)}<option value={option.value}
								>{option.label}</option
							>{/each}</select
					><Button
						variant="outline"
						size="sm"
						disabled={!editable()}
						onclick={connectFromForm}
						><Link2Icon data-icon="inline-start" />Connect ports</Button
					>
				</section>

				{#if selectedRunLoading}<section
						class="server-status"
						data-testid="automation-editor-run-loading"
					>
						<span>Run details</span><strong>Loading…</strong>
					</section>{:else if selectedRun}<section
						class="server-status"
						data-testid="automation-editor-server-status"
					>
						<span>Latest run</span><strong>{selectedRun.status}</strong>
					</section>{/if}<AutomationRunHistory
					{runs}
					{selectedRun}
					loading={selectedRunLoading}
					onSelect={onSelectRun}
				/>
			</div>
		</aside>
	</div>
</dialog>

<style>
	.automation-editor {
		position: fixed;
		inset: 0;
		z-index: 100;
		display: flex;
		width: 100%;
		height: 100%;
		max-width: none;
		max-height: none;
		min-height: 100svh;
		margin: 0;
		padding: 0;
		border: 0;
		flex-direction: column;
		overflow: hidden;
		background: var(--background);
		color: var(--foreground);
	}
	.automation-editor::backdrop {
		background: transparent;
	}
	.editor-header {
		display: flex;
		min-height: 4.2rem;
		align-items: center;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.75rem 1.25rem;
		border-bottom: 1px solid var(--border);
		background: color-mix(in oklch, var(--card) 94%, var(--background));
	}
	.editor-heading,
	.editor-actions,
	.editor-title-copy,
	.graph-status,
	.canvas-toolbar,
	.canvas-toolbar-actions,
	.section-heading,
	.inspector-actions,
	.palette-footer,
	.editor-notice,
	.editor-errors,
	.editor-feedback,
	.boundary-mark {
		display: flex;
		align-items: center;
	}
	.editor-heading,
	.editor-actions {
		gap: 0.75rem;
	}
	.editor-title-copy {
		min-width: 0;
		flex-direction: column;
		align-items: flex-start;
		gap: 0.12rem;
	}
	.editor-eyebrow,
	.panel-kicker,
	.canvas-label {
		font-size: 0.64rem;
		font-weight: 650;
		letter-spacing: 0.09em;
		line-height: 1.1;
		text-transform: uppercase;
		color: var(--muted-foreground);
	}
	.editor-title-copy h1 {
		max-width: min(32rem, 42vw);
		overflow: hidden;
		margin: 0;
		font-size: 1.12rem;
		font-weight: 650;
		letter-spacing: -0.02em;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.editor-actions {
		flex-wrap: wrap;
		justify-content: flex-end;
	}
	.graph-status {
		gap: 0.4rem;
		padding-inline: 0.25rem;
		font-size: 0.72rem;
		color: var(--muted-foreground);
		white-space: nowrap;
	}
	.status-dot {
		width: 0.45rem;
		height: 0.45rem;
		border-radius: 999px;
		background: var(--success);
	}
	.status-dot.status-dirty {
		background: var(--warning);
	}
	.editor-boundary {
		display: flex;
		align-items: flex-start;
		gap: 0.7rem;
		margin: 0.75rem 1.25rem 0;
		padding: 0.65rem 0.8rem;
		border: 1px solid color-mix(in oklch, var(--primary) 22%, var(--border));
		border-radius: var(--radius-md);
		background: color-mix(in oklch, var(--accent) 45%, var(--card));
		font-size: 0.72rem;
	}
	.editor-boundary > div:nth-child(2) {
		min-width: 0;
		flex: 1;
	}
	.boundary-mark {
		width: 1.8rem;
		height: 1.8rem;
		justify-content: center;
		flex: 0 0 auto;
		border-radius: 0.45rem;
		background: color-mix(in oklch, var(--primary) 13%, transparent);
		color: var(--primary);
	}
	.boundary-mark :global(svg),
	.editor-errors :global(svg),
	.editor-notice :global(svg),
	.editor-feedback :global(svg) {
		width: 0.9rem;
		height: 0.9rem;
		flex: 0 0 auto;
	}
	.editor-boundary strong {
		display: block;
		font-weight: 650;
		color: var(--foreground);
	}
	.editor-boundary p {
		margin: 0.2rem 0 0;
		line-height: 1.4;
		color: var(--muted-foreground);
	}
	.editor-boundary :global([data-slot='badge']) {
		align-self: center;
		white-space: nowrap;
	}
	.editor-errors,
	.editor-notice,
	.editor-feedback {
		gap: 0.55rem;
		margin: 0.65rem 1.25rem 0;
		padding: 0.6rem 0.75rem;
		border: 1px solid color-mix(in oklch, var(--destructive) 26%, var(--border));
		border-radius: var(--radius-md);
		background: color-mix(in oklch, var(--destructive) 7%, var(--card));
		font-size: 0.75rem;
		color: var(--destructive);
	}
	.editor-errors strong {
		font-weight: 650;
	}
	.editor-errors p {
		margin: 0.15rem 0 0;
		color: var(--muted-foreground);
	}
	.editor-notice {
		border-color: color-mix(in oklch, var(--warning) 30%, var(--border));
		background: color-mix(in oklch, var(--warning) 8%, var(--card));
		color: var(--warning);
	}
	.editor-feedback {
		border-color: color-mix(in oklch, var(--success) 25%, var(--border));
		background: color-mix(in oklch, var(--success) 7%, var(--card));
		color: var(--success);
	}
	.editor-body {
		display: grid;
		min-height: 0;
		flex: 1;
		grid-template-columns: minmax(13rem, 15.5rem) minmax(0, 1fr) minmax(18rem, 21rem);
		margin-top: 0.75rem;
		border-top: 1px solid var(--border);
	}
	.editor-palette,
	.editor-inspector {
		min-width: 0;
		min-height: 0;
		background: var(--card);
	}
	.editor-palette {
		display: flex;
		flex-direction: column;
		gap: 0.8rem;
		padding: 1rem 0.9rem;
		border-right: 1px solid var(--border);
	}
	.editor-inspector {
		display: flex;
		flex-direction: column;
		border-left: 1px solid var(--border);
	}
	.panel-heading {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 0.5rem;
	}
	.panel-heading h2,
	.section-heading h3 {
		margin: 0.18rem 0 0;
		font-size: 0.96rem;
		font-weight: 650;
		letter-spacing: -0.01em;
	}
	.panel-description,
	.field-help {
		margin: 0;
		font-size: 0.72rem;
		line-height: 1.5;
		color: var(--muted-foreground);
	}
	.palette-list {
		display: grid;
		gap: 0.45rem;
		overflow-y: auto;
	}
	.palette-item {
		display: flex;
		min-width: 0;
		align-items: center;
		gap: 0.55rem;
		padding: 0.55rem;
		border: 1px solid transparent;
		border-radius: var(--radius-md);
		background: transparent;
		color: var(--foreground);
		font: inherit;
		text-align: left;
		cursor: pointer;
		transition:
			background-color 150ms ease,
			border-color 150ms ease;
	}
	.palette-item:hover,
	.palette-item:focus-visible {
		border-color: color-mix(in oklch, var(--primary) 25%, var(--border));
		background: color-mix(in oklch, var(--accent) 40%, var(--card));
		outline: none;
	}
	.palette-icon {
		display: grid;
		width: 1.8rem;
		height: 1.8rem;
		place-items: center;
		flex: 0 0 auto;
		border-radius: 0.45rem;
		background: color-mix(in oklch, var(--primary) 10%, transparent);
		color: var(--primary);
	}
	.palette-icon[data-kind='trigger'] {
		background: color-mix(in oklch, var(--success) 12%, transparent);
		color: var(--success);
	}
	.palette-icon[data-kind='action'] {
		background: color-mix(in oklch, var(--info) 12%, transparent);
		color: var(--info);
	}
	.palette-icon[data-kind='condition'] {
		background: color-mix(in oklch, var(--warning) 14%, transparent);
		color: var(--warning);
	}
	.palette-icon[data-kind='human_gate'] {
		background: color-mix(in oklch, var(--destructive) 10%, transparent);
		color: var(--destructive);
	}
	.palette-icon :global(svg),
	.palette-add :global(svg) {
		width: 0.88rem;
		height: 0.88rem;
	}
	.palette-copy {
		display: flex;
		min-width: 0;
		flex: 1;
		flex-direction: column;
		gap: 0.16rem;
		align-items: flex-start;
	}
	.palette-copy strong {
		font-size: 0.75rem;
		font-weight: 600;
	}
	.palette-copy small {
		display: -webkit-box;
		overflow: hidden;
		font-size: 0.64rem;
		line-height: 1.35;
		color: var(--muted-foreground);
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
	}
	.palette-add {
		width: 1rem;
		height: 1rem;
		flex: 0 0 auto;
		color: var(--muted-foreground);
	}
	.palette-item:hover .palette-add,
	.palette-item:focus-visible .palette-add {
		color: var(--primary);
	}
	.palette-footer {
		align-items: flex-start;
		gap: 0.4rem;
		margin-top: auto;
		padding-top: 0.7rem;
		border-top: 1px solid var(--border);
		font-size: 0.65rem;
		line-height: 1.4;
		color: var(--muted-foreground);
	}
	.palette-footer :global(svg) {
		width: 0.85rem;
		height: 0.85rem;
		flex: 0 0 auto;
		color: var(--primary);
	}
	.editor-canvas {
		display: flex;
		min-width: 0;
		min-height: 0;
		flex-direction: column;
		background: color-mix(in oklch, var(--muted) 20%, var(--background));
	}
	.canvas-toolbar {
		min-height: 2.9rem;
		justify-content: space-between;
		gap: 1rem;
		padding: 0.45rem 0.8rem 0.45rem 1rem;
		border-bottom: 1px solid var(--border);
		background: color-mix(in oklch, var(--card) 88%, var(--background));
	}
	.canvas-toolbar > div:first-child {
		display: flex;
		align-items: baseline;
		gap: 0.6rem;
	}
	.canvas-meta {
		font-size: 0.68rem;
		color: var(--muted-foreground);
	}
	.canvas-toolbar-actions {
		gap: 0.2rem;
	}
	.flow-frame {
		position: relative;
		min-height: 0;
		flex: 1;
	}
	.flow-frame :global([data-testid='workflow-editor']) {
		width: 100%;
		height: 100%;
		min-height: 20rem;
	}
	.editor-loading {
		display: grid;
		height: 100%;
		min-height: 20rem;
		place-items: center;
		color: var(--muted-foreground);
		font-size: 0.8rem;
	}
	.inspector-scroll {
		min-height: 0;
		overflow-y: auto;
		padding: 1rem;
	}
	.inspector-section {
		display: flex;
		flex-direction: column;
		gap: 0.55rem;
		padding-bottom: 1rem;
		border-bottom: 1px solid var(--border);
	}
	.inspector-section + .inspector-section,
	.server-status {
		margin-top: 1rem;
	}
	.section-heading {
		align-items: flex-start;
		justify-content: space-between;
		gap: 0.5rem;
		margin-bottom: 0.25rem;
	}
	.section-heading h3 {
		font-size: 0.86rem;
	}
	.field-label {
		margin-top: 0.2rem;
		font-size: 0.69rem;
		font-weight: 600;
		color: var(--foreground);
	}
	.field-control {
		width: 100%;
		min-width: 0;
		min-height: 2.05rem;
		padding: 0.42rem 0.55rem;
		border: 1px solid var(--input);
		border-radius: var(--radius-sm);
		background: var(--background);
		color: var(--foreground);
		font: inherit;
		font-size: 0.74rem;
		outline: none;
	}
	.field-control:focus {
		border-color: var(--ring);
		box-shadow: 0 0 0 3px color-mix(in oklch, var(--ring) 20%, transparent);
	}
	.field-control[readonly] {
		background: color-mix(in oklch, var(--muted) 45%, var(--background));
		color: var(--muted-foreground);
	}
	.field-control[aria-invalid='true'] {
		border-color: var(--destructive);
	}
	.field-mono,
	.config-value {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 0.92em;
	}
	.field-error {
		font-size: 0.65rem;
		color: var(--destructive);
	}
	.config-fields {
		display: flex;
		flex-direction: column;
		gap: 0.48rem;
		margin-top: 0.25rem;
		padding-top: 0.7rem;
		border-top: 1px solid var(--border);
	}
	.config-field {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	.config-field > span {
		font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
		font-size: 0.62rem;
		color: var(--muted-foreground);
	}
	.config-value {
		overflow-wrap: anywhere;
		padding: 0.45rem;
		border-radius: var(--radius-sm);
		background: color-mix(in oklch, var(--muted) 45%, transparent);
		color: var(--foreground);
	}
	.inspector-actions {
		gap: 0.45rem;
		margin-top: 0.25rem;
	}
	.accessible-node-list {
		display: grid;
		gap: 0.35rem;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.accessible-node-list > li {
		min-width: 0;
	}
	.accessible-node {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.6rem;
		padding: 0.48rem 0.55rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
		background: var(--background);
		color: var(--foreground);
		text-align: left;
		cursor: pointer;
	}
	.accessible-node:hover,
	.accessible-node:focus-visible,
	.accessible-node.selected {
		border-color: var(--ring);
		background: color-mix(in oklch, var(--accent) 40%, var(--background));
		outline: none;
	}
	.accessible-node span:first-child {
		display: flex;
		min-width: 0;
		flex-direction: column;
		gap: 0.15rem;
	}
	.accessible-node strong {
		overflow: hidden;
		font-size: 0.72rem;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.accessible-node small {
		font-size: 0.62rem;
		color: var(--muted-foreground);
	}
	.server-status {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		padding: 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-md);
		background: color-mix(in oklch, var(--muted) 35%, var(--card));
		font-size: 0.72rem;
	}
	.server-status span {
		color: var(--muted-foreground);
	}
	@media (max-width: 1100px) {
		.editor-body {
			grid-template-columns: minmax(11rem, 13rem) minmax(0, 1fr);
		}
		.editor-inspector {
			grid-column: 1 / -1;
			max-height: 19rem;
			border-top: 1px solid var(--border);
			border-left: 0;
		}
	}
	@media (max-width: 720px) {
		.editor-header {
			align-items: flex-start;
			flex-direction: column;
		}
		.editor-actions {
			width: 100%;
			justify-content: flex-start;
			gap: 0.35rem;
		}
		.graph-status {
			order: -1;
			width: 100%;
		}
		.editor-body {
			display: flex;
			flex-direction: column;
			overflow-y: auto;
		}
		.editor-palette {
			flex: 0 0 auto;
			max-height: none;
			border-right: 0;
			border-bottom: 1px solid var(--border);
		}
		.palette-list {
			max-height: 15rem;
		}
		.editor-canvas {
			min-height: 26rem;
		}
		.editor-inspector {
			max-height: none;
		}
		.editor-boundary {
			margin-inline: 0.7rem;
			display: grid;
			grid-template-columns: auto minmax(0, 1fr);
		}
		.editor-boundary :global([data-slot='badge']) {
			grid-column: 2;
			justify-self: start;
			white-space: normal;
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.palette-item {
			transition: none;
		}
	}
</style>
