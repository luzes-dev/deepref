<script lang="ts">
	import { MediaQuery, SvelteMap, SvelteSet } from 'svelte/reactivity';
	import { onDestroy, onMount, tick, untrack } from 'svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
	import CheckIcon from '@lucide/svelte/icons/check';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import ClipboardPasteIcon from '@lucide/svelte/icons/clipboard-paste';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
	import FlaskConicalIcon from '@lucide/svelte/icons/flask-conical';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import RedoIcon from '@lucide/svelte/icons/redo-2';
	import ScissorsIcon from '@lucide/svelte/icons/scissors';
	import SendIcon from '@lucide/svelte/icons/send';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import SparklesIcon from '@lucide/svelte/icons/sparkles';
	import StickyNoteIcon from '@lucide/svelte/icons/sticky-note';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import UndoIcon from '@lucide/svelte/icons/undo-2';
	import XIcon from '@lucide/svelte/icons/x';
	import { BuilderCanvas, type BuilderCanvasApi, type FlowConnection } from '@deepref/ui/flow';
	import * as Dialog from '@deepref/ui/dialog';
	import * as DropdownMenu from '@deepref/ui/dropdown-menu';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { Switch } from '@deepref/ui/switch';
	import * as Tabs from '@deepref/ui/tabs';
	import {
		cancelWorkflowRun,
		disableWorkflow,
		enableWorkflow,
		getAutomationCatalog,
		getBlockDetails,
		getWorkflow,
		getWorkflowRun,
		publishWorkflow,
		startWorkflowTestRun,
		updateWorkflow,
		validateWorkflow
	} from '#lib/api/generated/automations/automations.js';
	import { listExtractionFields } from '#lib/api/generated/extraction/extraction.js';
	import type {
		FieldDetailsDto,
		NodeTypeDto,
		WorkflowDto,
		WorkflowRunDto
	} from '#lib/api/generated/models/index.js';
	import { notifyError, notifyInfo, notifySuccess } from '#lib/features/notifications/toast.js';
	import { graphOf, isActiveRun, isConflict, issuesFromError, type Issue } from './api';
	import { freeSpot, headerHelp, unavailableReason, type BuilderAction } from './builder-view';
	import CanvasMenu, { type MenuEntry } from './CanvasMenu.svelte';
	import DataView from './DataView.svelte';
	import { iconForNode } from './icons';
	import Inspector from './Inspector.svelte';
	import RunHistory from './RunHistory.svelte';
	import {
		autoArrange,
		blockData,
		catalogIndex,
		defaultConfig,
		describeTrigger,
		findPort,
		flowToGraph,
		graphToFlow,
		newNodeId,
		portsCompatible,
		previewForRun,
		refusalReason,
		sampleTriggerData,
		statusOfRuns,
		summarizeNode,
		type FlowNode,
		type WfGraph
	} from './model';
	import type { FlowGraphEdge as Edge } from '@deepref/ui/flow';

	let {
		projectId,
		workflowId,
		initialRunId = null
	}: { projectId: string; workflowId: string; initialRunId?: string | null } = $props();

	// -- state ---------------------------------------------------------------
	let loading = $state(true);
	let loadError = $state<string | null>(null);
	// Replaced as a whole, never edited in place, so Svelte Flow can copy the data it holds.
	let workflow = $state.raw<WorkflowDto | null>(null);
	let catalog = $state.raw<NodeTypeDto[]>([]);
	const defs = $derived(catalogIndex(catalog));
	let extractionFields = $state<{ id: string; label: string }[]>([]);

	let nodes = $state.raw<FlowNode[]>([]);
	let edges = $state.raw<Edge[]>([]);
	let name = $state('');
	let selectedId = $state<string | null>(null);
	let canvasApi: BuilderCanvasApi | null = null;
	let canvasBox = $state<HTMLDivElement | null>(null);

	let saveState = $state<'saved' | 'dirty' | 'saving' | 'error' | 'conflict'>('saved');
	let revision = 0;
	let saveTimer: ReturnType<typeof setTimeout> | undefined;
	let savePromise: Promise<boolean> = Promise.resolve(true);

	let issues = $state<Issue[]>([]);
	let showIssues = $state(false);
	let busy = $state<'validate' | 'publish' | 'test' | 'toggle' | null>(null);

	let tab = $state<'canvas' | 'history'>('canvas');
	// A finished or running test/run drawn over the canvas.
	let overlay = $state<{ run: WorkflowRunDto; kind: 'test' | 'run' } | null>(null);
	let editStash: { nodes: FlowNode[]; edges: Edge[] } | null = null;
	let pollTimer: ReturnType<typeof setInterval> | undefined;
	let testOpen = $state(false);
	// Phones get the block menu as a sheet and a header that fits the screen.
	const narrow = new MediaQuery('(max-width: 767px)');
	// The output port tapped to start a connection; the next input tapped completes it.
	let armed = $state<{ nodeId: string; portId: string } | null>(null);
	// The problems list starts collapsed so that it never covers the blocks.
	let issuesOpen = $state(false);
	let issuesHidden = $state(false);

	let menu = $state<{
		x: number;
		y: number;
		entries: MenuEntry[];
		startInAdd: boolean;
		accepts: string | null;
		flow: { x: number; y: number };
		from: { nodeId: string; portId: string } | null;
	} | null>(null);
	let hint = $state<{ x: number; y: number; text: string } | null>(null);
	let hintTimer: ReturnType<typeof setTimeout> | undefined;

	const readOnly = $derived(overlay?.kind === 'run');
	const hasTrigger = $derived(nodes.some((node) => node.data.typeId?.startsWith('trigger.')));
	const blockNodes = $derived(nodes.filter((node) => node.type === 'block'));
	// Opening the side panel narrows the canvas (or covers it on small screens):
	// pan so the selected block stays in view.
	$effect(() => {
		const id = selectedId;
		if (!id) return;
		const timer = setTimeout(() => {
			if (!canvasBox || !canvasApi) return;
			const overlay =
				typeof window !== 'undefined' && !window.matchMedia('(min-width: 768px)').matches;
			canvasApi.revealNode(id, {
				width: canvasBox.clientWidth,
				height: overlay ? canvasBox.clientHeight / 2 : canvasBox.clientHeight
			});
		}, 120);
		return () => clearTimeout(timer);
	});

	const selectedNode = $derived(nodes.find((node) => node.id === selectedId) ?? null);

	// What the selected block's fields can read from the steps before it. The server
	// works it out from the draft on screen, so a connection that is not saved yet
	// counts. Requests are debounced, and only the latest answer is kept.
	let blockDetails = $state<Record<string, FieldDetailsDto>>({});
	let blockDetailsRequest = 0;
	$effect(() => {
		const id = selectedId;
		const graph = flowToGraph(nodes, edges);
		const request = ++blockDetailsRequest;
		if (id === null) {
			blockDetails = {};
			return;
		}
		const timer = setTimeout(() => {
			getBlockDetails(projectId, { graph, node_id: id })
				.then((response) => {
					if (request === blockDetailsRequest) blockDetails = response.data.fields;
				})
				.catch(() => {
					if (request === blockDetailsRequest) blockDetails = {};
				});
		}, 300);
		return () => clearTimeout(timer);
	});
	const triggerType = $derived(
		blockNodes.find((n) => n.data.typeId.startsWith('trigger.'))?.data.typeId
	);
	const isManual = $derived(triggerType === 'trigger.manual');
	const controlState = $derived({
		readOnly,
		busy: busy !== null,
		conflict: saveState === 'conflict',
		blockCount: blockNodes.length,
		published: workflow?.published_version != null
	});
	/** Why a control is unavailable, for its tooltip. */
	const reasonFor = (action: BuilderAction) => unavailableReason(action, controlState);
	const help = $derived(headerHelp(controlState, isManual));
	/** The output an armed connection starts from, in words. */
	const armedFrom = $derived.by(() => {
		const current = armed;
		if (!current) return null;
		const node = nodes.find((item) => item.id === current.nodeId);
		const port = node?.data.outputs.find((item) => item.id === current.portId);
		return { step: node?.data.title ?? 'this step', output: port?.label ?? 'the output' };
	});
	const issuesByNode = $derived.by(() => {
		const map = new SvelteMap<string, string[]>();
		for (const issue of issues) {
			if (!issue.node_id) continue;
			map.set(issue.node_id, [...(map.get(issue.node_id) ?? []), issue.message]);
		}
		return map;
	});
	const generalIssues = $derived(issues.filter((issue) => !issue.node_id));

	// -- loading -------------------------------------------------------------
	async function load() {
		loading = true;
		loadError = null;
		try {
			const [catalogResponse, workflowResponse, fieldsResponse] = await Promise.all([
				getAutomationCatalog(),
				getWorkflow(projectId, workflowId),
				listExtractionFields(projectId).catch(() => null)
			]);
			catalog = catalogResponse.data;
			extractionFields = (fieldsResponse?.data ?? []).map((field) => ({
				id: field.id,
				label: field.label
			}));
			adopt(workflowResponse.data);
			loadNotes();
			history = [snapshot()];
			historyIndex = 0;
		} catch (error) {
			loadError =
				error instanceof Error ? error.message : 'This automation could not be opened.';
		} finally {
			loading = false;
		}
	}

	function adopt(next: WorkflowDto) {
		workflow = next;
		name = next.name;
		revision = next.draft_revision;
		const flow = graphToFlow(graphOf(next), defs.size ? defs : catalogIndex(catalog));
		nodes = flow.nodes;
		edges = flow.edges;
		saveState = 'saved';
	}

	onMount(() => {
		void load();
		return () => {
			clearTimeout(saveTimer);
			clearInterval(pollTimer);
			clearTimeout(hintTimer);
		};
	});

	onDestroy(() => {
		// Do not lose the last edits when leaving the page.
		if (saveState === 'dirty' && workflow && !readOnly) void saveNow();
	});

	// -- notes (kept in this browser; they are not part of the automation) ---
	const notesKey = $derived(`deepref.automation-notes.${workflowId}`);
	function loadNotes() {
		try {
			const raw = localStorage.getItem(notesKey);
			if (!raw) return;
			const notes = JSON.parse(raw) as { id: string; x: number; y: number; text: string }[];
			const restored: FlowNode[] = notes.map((note) => ({
				id: note.id,
				type: 'note',
				position: { x: note.x, y: note.y },
				data: {
					title: note.text,
					tone: 'note',
					inputs: [],
					outputs: [],
					typeId: 'note',
					config: {},
					customLabel: null
				}
			}));
			nodes = [...nodes, ...restored];
		} catch {
			// Notes are a convenience; ignore storage problems.
		}
	}
	function saveNotes() {
		try {
			const notes = nodes
				.filter((node) => node.type === 'note')
				.map((node) => ({
					id: node.id,
					x: Math.round(node.position.x),
					y: Math.round(node.position.y),
					text: node.data.title
				}));
			if (notes.length) localStorage.setItem(notesKey, JSON.stringify(notes));
			else localStorage.removeItem(notesKey);
		} catch {
			// ignore
		}
	}

	// -- history (undo / redo) -----------------------------------------------
	const HISTORY_LIMIT = 100;
	let history = $state<string[]>([]);
	let historyIndex = $state(0);
	let coalesceKey = '';
	let coalesceAt = 0;

	function snapshot(): string {
		return JSON.stringify({
			name,
			graph: flowToGraph(nodes, edges),
			notes: nodes
				.filter((node) => node.type === 'note')
				.map((node) => ({ id: node.id, position: node.position, text: node.data.title }))
		});
	}

	/** Remember the current state so it can be undone, and schedule a save. */
	function commit(key = '') {
		if (readOnly || loading) return;
		const now = Date.now();
		const current = snapshot();
		if (current === history[historyIndex]) return;
		const coalesce = key !== '' && key === coalesceKey && now - coalesceAt < 800;
		coalesceKey = key;
		coalesceAt = now;
		const base = history.slice(0, historyIndex + (coalesce ? 0 : 1));
		history = [...base, current].slice(-HISTORY_LIMIT);
		historyIndex = history.length - 1;
		markDirty();
		saveNotes();
	}

	function restore(index: number) {
		const entry = history[index];
		if (!entry || readOnly) return;
		const parsed = JSON.parse(entry) as {
			name: string;
			graph: WfGraph;
			notes: { id: string; position: { x: number; y: number }; text: string }[];
		};
		const flow = graphToFlow(parsed.graph, defs);
		const notes: FlowNode[] = parsed.notes.map((note) => ({
			id: note.id,
			type: 'note',
			position: note.position,
			data: {
				title: note.text,
				tone: 'note',
				inputs: [],
				outputs: [],
				typeId: 'note',
				config: {},
				customLabel: null
			}
		}));
		nodes = [...flow.nodes, ...notes];
		edges = flow.edges;
		name = parsed.name;
		historyIndex = index;
		if (selectedId && !nodes.some((node) => node.id === selectedId)) selectedId = null;
		markDirty();
		saveNotes();
	}

	const canUndo = $derived(historyIndex > 0);
	const canRedo = $derived(historyIndex < history.length - 1);
	function undo() {
		if (canUndo) restore(historyIndex - 1);
	}
	function redo() {
		if (canRedo) restore(historyIndex + 1);
	}

	// -- saving --------------------------------------------------------------
	function markDirty() {
		if (saveState === 'conflict') return;
		saveState = 'dirty';
		clearTimeout(saveTimer);
		saveTimer = setTimeout(() => void saveNow(), 1200);
	}

	function saveNow(): Promise<boolean> {
		clearTimeout(saveTimer);
		savePromise = savePromise.then(async () => {
			if (!workflow || saveState === 'conflict' || readOnly) return saveState !== 'error';
			if (saveState !== 'dirty') return true;
			saveState = 'saving';
			const sent = snapshot();
			try {
				const response = await updateWorkflow(projectId, workflowId, {
					name: name.trim() || workflow.name,
					graph: flowToGraph(nodes, edges),
					expected_revision: revision
				});
				revision = response.data.draft_revision;
				workflow = response.data;
				// Edits made while the request was in flight keep the dirty mark.
				if (snapshot() === sent) saveState = 'saved';
				else markDirty();
				if (showIssues) void runValidation(true);
				return true;
			} catch (error) {
				if (isConflict(error)) {
					saveState = 'conflict';
				} else {
					saveState = 'error';
					notifyError('Could not save your changes', error);
				}
				return false;
			}
		});
		return savePromise;
	}

	async function reloadFromServer() {
		const response = await getWorkflow(projectId, workflowId);
		adopt(response.data);
		history = [snapshot()];
		historyIndex = 0;
	}

	// -- editing -------------------------------------------------------------
	function patchNode(id: string, change: (node: FlowNode) => FlowNode) {
		nodes = nodes.map((node) => (node.id === id ? change(node) : node));
	}

	function setConfig(id: string, key: string, value: unknown) {
		patchNode(id, (node) => {
			const config = { ...node.data.config, [key]: value };
			const def = defs.get(node.data.typeId);
			return {
				...node,
				data: { ...node.data, config, summary: summarizeNode(def, config) }
			};
		});
		commit(`config:${id}:${key}`);
	}

	function rename(id: string, label: string) {
		patchNode(id, (node) => {
			const def = defs.get(node.data.typeId);
			const custom = label.trim() ? label : null;
			return {
				...node,
				data: {
					...node.data,
					customLabel: custom,
					title: custom?.trim() || def?.label || node.data.title,
					kind: custom?.trim() ? def?.label : undefined
				}
			};
		});
		commit(`label:${id}`);
	}

	function snap(value: number): number {
		return Math.round(value / 20) * 20;
	}

	function addBlock(
		def: NodeTypeDto,
		at: { x: number; y: number },
		from: { nodeId: string; portId: string } | null
	) {
		const id = newNodeId(
			def.id,
			nodes.map((node) => node.id)
		);
		const config = defaultConfig(def);
		const node: FlowNode = {
			id,
			type: 'block',
			position: { x: snap(at.x), y: snap(at.y) },
			data: blockData(def, def.id, config, null)
		};
		nodes = [
			...nodes.map((item) => ({ ...item, selected: false })),
			{ ...node, selected: !narrow.current }
		];
		if (from) {
			const source = nodes.find((item) => item.id === from.nodeId);
			const sourcePort = findPort(defs.get(source?.data.typeId ?? ''), 'out', from.portId);
			const target = def.inputs.find(
				(input) => sourcePort && portsCompatible(sourcePort.type, input.type)
			);
			if (target) {
				edges = [
					...edges,
					{
						id: `${from.nodeId}:${from.portId}->${id}:${target.id}`,
						source: from.nodeId,
						sourceHandle: from.portId,
						target: id,
						targetHandle: target.id
					}
				];
			}
		}
		// On a phone the new block is not selected: its settings sheet would cover
		// the canvas, and the ports are needed to connect the block.
		selectedId = narrow.current ? null : id;
		commit();
		if (from) void tick().then(() => setTimeout(() => canvasApi?.fitView(), 60));
	}

	function deleteNodes(ids: string[]) {
		const gone = new SvelteSet(ids);
		nodes = nodes.filter((node) => !gone.has(node.id));
		edges = edges.filter((edge) => !gone.has(edge.source) && !gone.has(edge.target));
		if (selectedId && gone.has(selectedId)) selectedId = null;
		commit();
	}

	function deleteEdges(ids: string[]) {
		const gone = new SvelteSet(ids);
		edges = edges.filter((edge) => !gone.has(edge.id));
		commit();
	}

	function disconnect(id: string) {
		edges = edges.filter((edge) => edge.source !== id && edge.target !== id);
		commit();
	}

	function addNote(at: { x: number; y: number }) {
		const id = `note_${Date.now().toString(36)}`;
		const note: FlowNode = {
			id,
			type: 'note',
			position: { x: snap(at.x), y: snap(at.y) },
			data: {
				title: 'Write a note…',
				tone: 'note',
				inputs: [],
				outputs: [],
				typeId: 'note',
				config: {},
				customLabel: null
			}
		};
		nodes = [
			...nodes.map((item) => ({ ...item, selected: false })),
			{ ...note, selected: true }
		];
		selectedId = id;
		commit();
	}

	function setNoteText(id: string, text: string) {
		patchNode(id, (node) => ({ ...node, data: { ...node.data, title: text } }));
		commit(`note:${id}`);
	}

	// copy / paste / duplicate
	let clipboard: { nodes: FlowNode[]; edges: Edge[] } | null = null;
	function copyNodes(ids: string[]) {
		const chosen = new SvelteSet(ids);
		const picked = nodes.filter((node) => chosen.has(node.id));
		if (!picked.length) return;
		clipboard = {
			nodes: picked,
			edges: edges.filter((edge) => chosen.has(edge.source) && chosen.has(edge.target))
		};
	}
	function pasteNodes(at?: { x: number; y: number }) {
		if (!clipboard || readOnly) return;
		const first = clipboard.nodes[0];
		const dx = at ? at.x - first.position.x : 40;
		const dy = at ? at.y - first.position.y : 40;
		const taken = nodes.map((node) => node.id);
		const mapping = new SvelteMap<string, string>();
		const created: FlowNode[] = [];
		for (const node of clipboard.nodes) {
			if (node.data.typeId?.startsWith('trigger.') && hasTrigger) continue;
			const id =
				node.type === 'note'
					? `note_${Date.now().toString(36)}${created.length}`
					: newNodeId(node.data.typeId, [...taken, ...created.map((item) => item.id)]);
			mapping.set(node.id, id);
			created.push({
				...node,
				id,
				selected: true,
				position: { x: snap(node.position.x + dx), y: snap(node.position.y + dy) },
				data: {
					...node.data,
					config: structuredClone($state.snapshot(node.data.config)),
					issues: undefined,
					status: undefined,
					preview: undefined
				}
			});
		}
		if (!created.length) {
			notifyInfo('An automation can only have one trigger.');
			return;
		}
		const copiedEdges: Edge[] = clipboard.edges
			.filter((edge) => mapping.has(edge.source) && mapping.has(edge.target))
			.map((edge) => ({
				...edge,
				id: `${mapping.get(edge.source)}:${edge.sourceHandle}->${mapping.get(edge.target)}:${edge.targetHandle}`,
				source: mapping.get(edge.source)!,
				target: mapping.get(edge.target)!
			}));
		nodes = [...nodes.map((node) => ({ ...node, selected: false })), ...created];
		edges = [...edges, ...copiedEdges];
		selectedId = created.length === 1 ? created[0].id : null;
		commit();
	}
	function duplicate(id: string) {
		copyNodes([id]);
		pasteNodes();
	}

	function arrange() {
		const moved = autoArrange(
			nodes.filter((node) => node.type === 'block'),
			edges
		);
		const positions = new SvelteMap(moved.map((node) => [node.id, node.position]));
		nodes = nodes.map((node) => ({
			...node,
			position: positions.get(node.id) ?? node.position
		}));
		commit();
		void tick().then(() => canvasApi?.fitView());
	}

	// -- connections ---------------------------------------------------------
	function reaches(from: string, to: string): boolean {
		const seen = new SvelteSet<string>();
		const stack = [from];
		while (stack.length) {
			const id = stack.pop()!;
			if (id === to) return true;
			if (seen.has(id)) continue;
			seen.add(id);
			for (const edge of edges) if (edge.source === id) stack.push(edge.target);
		}
		return false;
	}

	/** Returns a plain-language reason when two ports must not be connected. */
	function connectionProblem(
		sourceId: string,
		sourcePort: string,
		targetId: string,
		targetPort: string
	): string | null {
		if (sourceId === targetId) return 'A step cannot be connected to itself.';
		const source = findPort(
			defs.get(nodes.find((n) => n.id === sourceId)?.data.typeId ?? ''),
			'out',
			sourcePort
		);
		const targetDef = defs.get(nodes.find((n) => n.id === targetId)?.data.typeId ?? '');
		const target = findPort(targetDef, 'in', targetPort);
		if (!source || !target) return 'These two points cannot be connected.';
		if (!portsCompatible(source.type, target.type)) return refusalReason(source, target);
		if (
			!target.multiple &&
			edges.some((edge) => edge.target === targetId && edge.targetHandle === targetPort)
		) {
			return `"${target.label}" already has a connection. Remove it first to connect a different step.`;
		}
		if (reaches(targetId, sourceId))
			return 'This would make the automation loop back on itself.';
		return null;
	}

	function showHint(text: string, x: number, y: number) {
		hint = { text, x, y };
		clearTimeout(hintTimer);
		hintTimer = setTimeout(() => (hint = null), 4500);
	}

	// -- menus ---------------------------------------------------------------
	function flowPoint(clientX: number, clientY: number) {
		return canvasApi?.screenToFlow({ x: clientX, y: clientY }) ?? { x: 0, y: 0 };
	}

	function openPaneMenu(event: MouseEvent) {
		if (readOnly) return;
		const flow = flowPoint(event.clientX, event.clientY);
		menu = {
			x: event.clientX,
			y: event.clientY,
			startInAdd: false,
			accepts: null,
			flow,
			from: null,
			entries: [
				{ kind: 'add', label: 'Add block', icon: PlusIcon },
				{
					kind: 'item',
					id: 'paste',
					label: 'Paste',
					icon: ClipboardPasteIcon,
					shortcut: 'Ctrl+V',
					disabled: !clipboard,
					onselect: () => pasteNodes(flow)
				},
				{
					kind: 'item',
					id: 'note',
					label: 'Add note',
					icon: StickyNoteIcon,
					onselect: () => addNote(flow)
				},
				{
					kind: 'item',
					id: 'arrange',
					label: 'Auto-arrange',
					icon: SparklesIcon,
					onselect: arrange
				},
				{ kind: 'separator' },
				{
					kind: 'item',
					id: 'test',
					label: 'Test from start',
					icon: FlaskConicalIcon,
					disabled: blockNodes.length === 0,
					onselect: () => (testOpen = true)
				}
			]
		};
	}

	function openNodeMenu(id: string, event: MouseEvent) {
		if (readOnly) return;
		const node = nodes.find((item) => item.id === id);
		if (!node) return;
		selectedId = id;
		const isNote = node.type === 'note';
		const isTrigger = node.data.typeId?.startsWith('trigger.');
		menu = {
			x: event.clientX,
			y: event.clientY,
			startInAdd: false,
			accepts: null,
			flow: flowPoint(event.clientX, event.clientY),
			from: null,
			entries: [
				{
					kind: 'item',
					id: 'rename',
					label: isNote ? 'Edit text' : 'Rename',
					icon: PencilIcon,
					onselect: () => {
						selectedId = id;
						void tick().then(() =>
							document
								.querySelector<HTMLInputElement>('#block-name, #note-text')
								?.focus()
						);
					}
				},
				{
					kind: 'item',
					id: 'duplicate',
					label: 'Duplicate',
					icon: CopyIcon,
					shortcut: 'Ctrl+C, Ctrl+V',
					disabled: isTrigger,
					onselect: () => duplicate(id)
				},
				...(isNote
					? []
					: [
							{
								kind: 'item' as const,
								id: 'disconnect',
								label: 'Disconnect',
								icon: ScissorsIcon,
								disabled: !edges.some(
									(edge) => edge.source === id || edge.target === id
								),
								onselect: () => disconnect(id)
							},
							{
								kind: 'item' as const,
								id: 'test-here',
								label: 'Test and look at this step',
								icon: FlaskConicalIcon,
								onselect: () => {
									selectedId = id;
									testOpen = true;
								}
							}
						]),
				{ kind: 'separator' },
				{
					kind: 'item',
					id: 'delete',
					label: 'Delete',
					icon: Trash2Icon,
					shortcut: 'Del',
					danger: true,
					onselect: () => deleteNodes([id])
				}
			]
		};
	}

	function openEdgeMenu(id: string, event: MouseEvent) {
		if (readOnly) return;
		menu = {
			x: event.clientX,
			y: event.clientY,
			startInAdd: false,
			accepts: null,
			flow: flowPoint(event.clientX, event.clientY),
			from: null,
			entries: [
				{
					kind: 'item',
					id: 'delete-edge',
					label: 'Delete connection',
					icon: Trash2Icon,
					danger: true,
					onselect: () => deleteEdges([id])
				}
			]
		};
	}

	function openAddFrom(nodeId: string, portId: string, point: { x: number; y: number }) {
		if (readOnly) return;
		const node = nodes.find((item) => item.id === nodeId);
		const port = findPort(defs.get(node?.data.typeId ?? ''), 'out', portId);
		const siblings = edges.filter((edge) => edge.source === nodeId).length;
		const flow = node
			? { x: node.position.x + 340, y: node.position.y + siblings * 170 }
			: flowPoint(point.x, point.y);
		menu = {
			x: point.x,
			y: point.y,
			entries: [],
			startInAdd: true,
			accepts: port?.type ?? null,
			flow,
			from: { nodeId, portId }
		};
	}

	function pickFromMenu(def: NodeTypeDto) {
		if (!menu) return;
		const { flow, from } = menu;
		menu = null;
		addBlock(def, flow, from);
	}

	/** Size of a block on the canvas, in canvas units (before measuring). */
	const BLOCK_SIZE = { width: 256, height: 120 };

	/**
	 * Opens the block menu for a block added from the middle of what is on
	 * screen. This is the way to add a block on a touch screen.
	 */
	function openAddAtCenter() {
		if (readOnly || !canvasBox) return;
		const box = canvasBox.getBoundingClientRect();
		const middle = flowPoint(box.left + box.width / 2, box.top + box.height / 2);
		const taken = nodes.map((node) => ({
			x: node.position.x,
			y: node.position.y,
			width: node.measured?.width ?? BLOCK_SIZE.width,
			height: node.measured?.height ?? BLOCK_SIZE.height
		}));
		const spot = freeSpot(
			{
				x: snap(middle.x - BLOCK_SIZE.width / 2),
				y: snap(middle.y - BLOCK_SIZE.height / 2)
			},
			taken,
			BLOCK_SIZE
		);
		menu = {
			x: box.left,
			y: box.top,
			entries: [],
			startInAdd: true,
			accepts: null,
			flow: spot,
			from: null
		};
	}

	// -- run overlays --------------------------------------------------------
	function applyOverlay() {
		const run = overlay?.run ?? null;
		const connected = new SvelteMap<string, string[]>();
		for (const edge of edges) {
			connected.set(edge.source, [
				...(connected.get(edge.source) ?? []),
				edge.sourceHandle ?? ''
			]);
		}
		const byNode = new SvelteMap<string, WorkflowRunDto['nodes']>();
		for (const entry of run?.nodes ?? []) {
			byNode.set(entry.node_id, [...(byNode.get(entry.node_id) ?? []), entry]);
		}
		const next = untrack(() => nodes).map((node) => {
			if (node.type !== 'block') return node;
			const entries = byNode.get(node.id) ?? [];
			const def = untrack(() => defs).get(node.data.typeId);
			const status = run
				? (statusOfRuns(entries) ?? (isActiveRun(run.status) ? 'queued' : 'skipped'))
				: undefined;
			// Plain data only: functions cannot be copied, so the "+" button's
			// handler is provided by the canvas, not stored on each block.
			const data = {
				...node.data,
				issues: issuesByNode.get(node.id) ?? [],
				connectedOutputs: connected.get(node.id) ?? [],
				status: status as typeof node.data.status,
				preview: run ? previewForRun(def, entries) : undefined,
				addable: !readOnly
			};
			const same =
				JSON.stringify([
					data.issues,
					data.connectedOutputs,
					data.status,
					data.preview,
					data.addable
				]) ===
				JSON.stringify([
					node.data.issues,
					node.data.connectedOutputs,
					node.data.status,
					node.data.preview,
					node.data.addable
				]);
			return same ? node : { ...node, data };
		});
		if (next.some((node, index) => node !== untrack(() => nodes)[index])) nodes = next;
	}

	$effect(() => {
		void issuesByNode;
		void overlay;
		void readOnly;
		void edges
			.map((edge) => `${edge.source}:${edge.sourceHandle}`)
			.sort()
			.join(',');
		void nodes.length;
		applyOverlay();
	});

	function clearOverlay() {
		clearInterval(pollTimer);
		pollTimer = undefined;
		if (overlay?.kind === 'run' && editStash) {
			nodes = editStash.nodes;
			edges = editStash.edges;
			editStash = null;
		}
		overlay = null;
	}

	function watchRun(runId: string) {
		clearInterval(pollTimer);
		pollTimer = setInterval(async () => {
			try {
				const response = await getWorkflowRun(projectId, runId);
				if (overlay && overlay.run.id === runId)
					overlay = { ...overlay, run: response.data };
				if (!isActiveRun(response.data.status)) clearInterval(pollTimer);
			} catch {
				clearInterval(pollTimer);
			}
		}, 1000);
	}

	async function openRun(run: WorkflowRunDto) {
		try {
			const full = (await getWorkflowRun(projectId, run.id)).data;
			if (!editStash) editStash = { nodes, edges };
			const flow = graphToFlow(graphOf(full), defs);
			nodes = flow.nodes;
			edges = flow.edges;
			overlay = { run: full, kind: 'run' };
			selectedId = null;
			tab = 'canvas';
			if (isActiveRun(full.status)) watchRun(full.id);
			await tick();
			canvasApi?.fitView();
		} catch (error) {
			notifyError('Could not open this run', error);
		}
	}

	// A notification links to one run (`?run=`): open it once the workflow has
	// loaded, and again when a different run is linked while the builder is open.
	let openedRunId: string | null = null;
	async function openRunById(runId: string): Promise<void> {
		try {
			await openRun((await getWorkflowRun(projectId, runId)).data);
		} catch (error) {
			notifyError('Could not open this run', error);
		}
	}
	$effect(() => {
		const runId = initialRunId;
		if (!runId || loading || runId === openedRunId) return;
		untrack(() => {
			openedRunId = runId;
			void openRunById(runId);
		});
	});

	async function stopRun() {
		if (!overlay) return;
		try {
			await cancelWorkflowRun(projectId, overlay.run.id);
			notifyInfo('Stopping the run…');
			watchRun(overlay.run.id);
		} catch (error) {
			notifyError('Could not stop the run', error);
		}
	}

	// -- validate / publish / test / toggle ----------------------------------
	async function runValidation(quiet = false): Promise<boolean> {
		if (!quiet) busy = 'validate';
		try {
			const report = (await validateWorkflow(projectId, workflowId)).data;
			issues = report.issues;
			showIssues = true;
			// A fresh check brings the problems pill back, even if it was dismissed.
			issuesHidden = false;
			issuesOpen = false;
			if (!quiet) {
				if (report.ok) notifySuccess('Everything looks good', 'No problems found.');
				else
					notifyInfo(
						`${report.issues.length} ${report.issues.length === 1 ? 'thing needs' : 'things need'} your attention`
					);
			}
			return report.ok;
		} catch (error) {
			if (!quiet) notifyError('Could not check the automation', error);
			return false;
		} finally {
			if (!quiet) busy = null;
		}
	}

	async function validateClick() {
		if (!(await saveNow())) return;
		await runValidation();
	}

	async function publish() {
		busy = 'publish';
		try {
			if (!(await saveNow())) return;
			const firstTime = workflow?.published_version == null;
			await publishWorkflow(projectId, workflowId, {});
			issues = [];
			showIssues = false;
			let next = (await getWorkflow(projectId, workflowId)).data;
			if (firstTime && next.status !== 'enabled') {
				next = (await enableWorkflow(projectId, workflowId)).data;
				notifySuccess('Published and turned on');
			} else {
				notifySuccess('Published');
			}
			workflow = next;
		} catch (error) {
			const problems = issuesFromError(error);
			if (problems) {
				issues = problems;
				showIssues = true;
				notifyInfo('Fix the highlighted steps, then publish again.');
			} else {
				notifyError('Could not publish', error);
			}
		} finally {
			busy = null;
		}
	}

	async function toggle(on: boolean) {
		if (!workflow) return;
		busy = 'toggle';
		try {
			workflow = (
				on
					? await enableWorkflow(projectId, workflowId)
					: await disableWorkflow(projectId, workflowId)
			).data;
		} catch (error) {
			// Show the server's own reason (for example "Publish the automation first.").
			notifyError(on ? 'Could not turn it on' : 'Could not turn it off', error);
		} finally {
			busy = null;
		}
	}

	async function runTest() {
		testOpen = false;
		busy = 'test';
		try {
			if (!(await saveNow())) return;
			const response = await startWorkflowTestRun(projectId, workflowId, {
				trigger_data: sampleTriggerData(triggerType),
				wait_ms: 20000
			});
			if (readOnly) clearOverlay();
			overlay = { run: response.data, kind: 'test' };
			if (isActiveRun(response.data.status)) watchRun(response.data.id);
			issues = [];
			showIssues = false;
			if (response.data.status === 'failed') {
				notifyInfo('The test stopped at a problem. Look for the red step.');
			}
		} catch (error) {
			const problems = issuesFromError(error);
			if (problems) {
				issues = problems;
				showIssues = true;
				notifyInfo('Fix the highlighted steps before testing.');
			} else {
				notifyError('Could not run the test', error);
			}
		} finally {
			busy = null;
		}
	}

	const selectedRuns = $derived(
		selectedId && overlay
			? overlay.run.nodes.filter((entry) => entry.node_id === selectedId)
			: []
	);

	// -- keyboard ------------------------------------------------------------
	function onKeydown(event: KeyboardEvent) {
		const target = event.target as HTMLElement | null;
		if (
			target &&
			(['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName) || target.isContentEditable)
		) {
			return;
		}
		if (menu) return;
		const mod = event.ctrlKey || event.metaKey;
		if (!mod) return;
		const key = event.key.toLowerCase();
		if (key === 'z') {
			event.preventDefault();
			if (event.shiftKey) redo();
			else undo();
		} else if (key === 'y') {
			event.preventDefault();
			redo();
		} else if (key === 's') {
			event.preventDefault();
			void saveNow();
		} else if (key === 'c' && !readOnly) {
			const ids = nodes.filter((node) => node.selected).map((node) => node.id);
			if (ids.length) {
				copyNodes(ids);
				event.preventDefault();
			}
		} else if (key === 'v' && !readOnly) {
			event.preventDefault();
			pasteNodes();
		}
	}

	const statusText = $derived(
		saveState === 'saving'
			? 'Saving…'
			: saveState === 'dirty'
				? 'Unsaved changes'
				: saveState === 'error'
					? 'Not saved'
					: saveState === 'conflict'
						? 'Changed elsewhere'
						: 'All changes saved'
	);
	const liveText = $derived.by(() => {
		if (!workflow) return '';
		if (workflow.published_version == null) return 'Not published yet';
		return workflow.has_unpublished_changes
			? `Published · version ${workflow.published_version} has newer edits`
			: `Published · version ${workflow.published_version}`;
	});
	const triggerText = $derived(
		describeTrigger({ nodes: flowToGraph(nodes, edges).nodes, edges: [] })
	);
</script>

<svelte:window onkeydown={onKeydown} />

<div class="flex h-full min-h-0 flex-col bg-background" data-testid="automation-builder">
	{#if loading}
		<div class="flex flex-1 items-center justify-center gap-2 text-sm text-muted-foreground">
			<Spinner /> Opening automation…
		</div>
	{:else if loadError || !workflow}
		<div class="flex flex-1 flex-col items-center justify-center gap-3 p-6 text-center">
			<p class="text-sm text-muted-foreground">
				{loadError ?? 'This automation could not be found.'}
			</p>
			<Button
				variant="outline"
				onclick={() => goto(resolve('/projects/[projectId]/automations', { projectId }))}
			>
				Back to automations
			</Button>
		</div>
	{:else}
		{#snippet viewTabs()}
			<Tabs.Root bind:value={tab}>
				<Tabs.List>
					<Tabs.Trigger value="canvas">Canvas</Tabs.Trigger>
					<Tabs.Trigger value="history">Run history</Tabs.Trigger>
				</Tabs.List>
			</Tabs.Root>
		{/snippet}

		{#snippet undoRedo()}
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label="Undo"
				title="Undo (Ctrl+Z)"
				disabled={!canUndo || readOnly}
				onclick={undo}><UndoIcon /></Button
			>
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label="Redo"
				title="Redo (Ctrl+Shift+Z)"
				disabled={!canRedo || readOnly}
				onclick={redo}><RedoIcon /></Button
			>
		{/snippet}

		{#snippet onSwitch()}
			{#if isManual}
				<span class="text-xs whitespace-nowrap text-muted-foreground md:text-sm"
					>Runs when you press Run</span
				>
			{:else}
				<label
					class="flex items-center gap-2 text-sm"
					title={reasonFor('switch') ?? undefined}
				>
					<Switch
						checked={workflow?.status === 'enabled'}
						disabled={reasonFor('switch') !== null}
						onCheckedChange={toggle}
						aria-label="Turn this automation on"
					/>
					{workflow?.status === 'enabled' ? 'On' : 'Off'}
				</label>
			{/if}
		{/snippet}

		{#snippet menuAction(label: string, reason: string | null, onselect: () => void)}
			<DropdownMenu.Item disabled={reason !== null} onclick={onselect}>
				<span class="flex min-w-0 flex-col">
					<span>{label}</span>
					{#if reason}
						<span class="text-2xs whitespace-normal text-muted-foreground"
							>{reason}</span
						>
					{/if}
				</span>
			</DropdownMenu.Item>
		{/snippet}

		<header class="border-b border-border bg-card" data-testid="builder-header">
			<div
				class="flex flex-wrap items-center gap-x-2 gap-y-1.5 px-2 py-1.5 md:gap-x-3 md:px-3 md:py-2"
			>
				<Button
					variant="ghost"
					size="icon-sm"
					aria-label="Back to automations"
					onclick={() =>
						goto(resolve('/projects/[projectId]/automations', { projectId }))}
				>
					<ArrowLeftIcon />
				</Button>
				<div class="flex min-w-0 flex-1 flex-col md:min-w-48">
					<input
						aria-label="Automation name"
						class="h-8 min-w-0 rounded-md border border-transparent bg-transparent px-1.5 text-base font-semibold outline-none hover:border-input focus-visible:border-input focus-visible:outline-2 focus-visible:outline-ring"
						value={name}
						disabled={readOnly}
						oninput={(event) => {
							name = event.currentTarget.value;
							commit('name');
						}}
					/>
					<p
						class="truncate px-1.5 text-2xs text-muted-foreground md:text-xs"
						data-testid="builder-status"
					>
						{statusText} · {liveText}
					</p>
				</div>

				{#if narrow.current}
					<Button
						size="sm"
						disabled={reasonFor('publish') !== null}
						title={reasonFor('publish') ?? undefined}
						onclick={publish}
					>
						{#if busy === 'publish'}<Spinner />{:else}<SendIcon />{/if} Publish
					</Button>
					<DropdownMenu.Root>
						<DropdownMenu.Trigger>
							{#snippet child({ props })}
								<Button
									{...props}
									variant="ghost"
									size="icon-sm"
									aria-label="More actions"
								>
									<EllipsisIcon />
								</Button>
							{/snippet}
						</DropdownMenu.Trigger>
						<DropdownMenu.Content align="end" class="w-64">
							{@render menuAction('Save draft', reasonFor('save'), () => saveNow())}
							{@render menuAction('Check', reasonFor('check'), validateClick)}
							{@render menuAction('Test', reasonFor('test'), () => (testOpen = true))}
						</DropdownMenu.Content>
					</DropdownMenu.Root>
				{:else}
					{@render viewTabs()}
					{@render undoRedo()}
					<Button
						variant="outline"
						size="sm"
						disabled={reasonFor('save') !== null}
						title={reasonFor('save') ?? undefined}
						onclick={() => saveNow()}
					>
						<CheckIcon /> Save draft
					</Button>
					<Button
						variant="outline"
						size="sm"
						disabled={reasonFor('check') !== null}
						title={reasonFor('check') ?? undefined}
						onclick={validateClick}
					>
						{#if busy === 'validate'}<Spinner />{:else}<ShieldCheckIcon />{/if} Check
					</Button>
					<Button
						variant="outline"
						size="sm"
						disabled={reasonFor('test') !== null}
						title={reasonFor('test') ?? undefined}
						onclick={() => (testOpen = true)}
					>
						{#if busy === 'test'}<Spinner />{:else}<FlaskConicalIcon />{/if} Test
					</Button>
					<Button
						variant="outline"
						size="sm"
						disabled={reasonFor('add') !== null}
						title={reasonFor('add') ?? 'Add a block to the canvas'}
						onclick={openAddAtCenter}
						data-testid="add-block"
					>
						<PlusIcon /> Add block
					</Button>
					<Button
						size="sm"
						disabled={reasonFor('publish') !== null}
						title={reasonFor('publish') ?? undefined}
						onclick={publish}
					>
						{#if busy === 'publish'}<Spinner />{:else}<SendIcon />{/if} Publish
					</Button>
					{@render onSwitch()}
				{/if}
			</div>
			{#if narrow.current}
				<div class="flex items-center gap-1 px-2 pb-1.5">
					{@render viewTabs()}
					{@render undoRedo()}
					<span class="flex-1"></span>
					{@render onSwitch()}
				</div>
			{/if}
			{#if help}
				<p
					class="px-2 pb-1.5 text-2xs text-muted-foreground md:px-3 md:text-xs"
					data-testid="builder-help"
				>
					{help}
				</p>
			{/if}
		</header>

		{#if saveState === 'conflict'}
			<div
				class="flex items-center gap-3 border-b border-warning-border bg-warning-surface px-3 py-2 text-sm"
			>
				This automation was changed somewhere else. Your last edits were not saved.
				<Button size="sm" variant="outline" onclick={reloadFromServer}
					>Load the latest version</Button
				>
			</div>
		{/if}

		{#if overlay}
			<div
				class="flex flex-wrap items-center gap-3 border-b border-border bg-info-surface px-3 py-2 text-sm"
				data-testid="run-banner"
			>
				<span class="font-medium">
					{overlay.kind === 'test' ? 'Test run' : 'Past run'} ·
					{overlay.run.status === 'completed'
						? 'finished'
						: overlay.run.status === 'failed'
							? 'stopped by a problem'
							: overlay.run.status === 'cancelled'
								? 'cancelled'
								: 'running…'}
				</span>
				{#if overlay.run.error}<span class="text-destructive">{overlay.run.error}</span
					>{/if}
				{#if overlay.kind === 'test'}
					<span class="text-muted-foreground">
						Steps that would change something only pretend to run.
					</span>
				{/if}
				<span class="flex-1"></span>
				{#if isActiveRun(overlay.run.status)}
					<Button size="sm" variant="outline" onclick={stopRun}>Stop</Button>
				{/if}
				<Button size="sm" variant="outline" onclick={clearOverlay}>
					{overlay.kind === 'test' ? 'Clear test results' : 'Back to editing'}
				</Button>
			</div>
		{/if}

		<div class="relative flex min-h-0 flex-1">
			{#if tab === 'history'}
				<div class="min-w-0 flex-1 overflow-auto p-4">
					<RunHistory {projectId} {workflowId} onopen={openRun} />
				</div>
			{:else}
				<div class="relative min-w-0 flex-1" bind:this={canvasBox}>
					<BuilderCanvas
						bind:nodes
						bind:edges
						{readOnly}
						fitOnLoad={nodes.length > 0}
						iconFor={(key) => iconForNode(key, defs.get(key)?.category)}
						onready={(api) => (canvasApi = api)}
						isValidConnection={(connection: FlowConnection) =>
							connectionProblem(
								connection.source,
								connection.sourceHandle ?? '',
								connection.target,
								connection.targetHandle ?? ''
							) === null}
						onconnect={() => {
							armed = null;
							void tick().then(() => commit());
						}}
						onconnectrefused={(event) => {
							const problem = connectionProblem(
								event.from.nodeId,
								event.from.portId,
								event.to.nodeId,
								event.to.portId
							);
							if (problem) showHint(problem, event.clientX, event.clientY);
						}}
						onconnectdrop={(event) =>
							openAddFrom(event.nodeId, event.portId, {
								x: event.clientX,
								y: event.clientY
							})}
						onarm={(from) => {
							armed = from;
							// A settings sheet would cover the ports the connection needs.
							if (from) selectedId = null;
						}}
						onportadd={(nodeId, portId, anchor) => {
							const box = anchor.getBoundingClientRect();
							openAddFrom(nodeId, portId, { x: box.right + 8, y: box.top });
						}}
						onnodeselect={(id) => {
							// Tapping the target port selects its block; keep the canvas clear.
							if (!armed) selectedId = id;
						}}
						onpaneclick={() => {
							menu = null;
							selectedId = null;
						}}
						onnodecontextmenu={openNodeMenu}
						onpanecontextmenu={openPaneMenu}
						onedgecontextmenu={openEdgeMenu}
						onchange={() => commit()}
						ondeletenodes={(ids) =>
							void tick().then(() => {
								if (selectedId && ids.includes(selectedId)) selectedId = null;
								commit();
							})}
						ondeleteedges={() => void tick().then(() => commit())}
					/>

					{#if armedFrom}
						<p
							role="status"
							class="pointer-events-none absolute inset-x-2 top-2 z-10 mx-auto w-fit max-w-full rounded-md border border-border bg-popover px-3 py-1.5 text-center text-xs text-popover-foreground shadow-md"
							data-testid="connect-armed"
						>
							Connecting from “{armedFrom.output}” on “{armedFrom.step}”. Tap an input
							on another step. Tap the output again to cancel.
						</p>
					{/if}

					{#if blockNodes.length === 0}
						<div
							class="pointer-events-none absolute inset-0 flex items-center justify-center p-6 text-center"
						>
							<div
								class="max-w-sm rounded-lg border border-dashed border-border bg-card/90 p-6"
							>
								<p class="text-sm font-medium">Start with a block</p>
								<p class="mt-1 text-sm text-muted-foreground">
									Tap “Add block” to choose a trigger, the step that starts this
									automation. On a computer you can also right-click the grid.
								</p>
							</div>
						</div>
					{/if}

					{#if narrow.current && !readOnly}
						<div class="absolute right-2 bottom-2 z-10">
							<Button
								size="lg"
								disabled={reasonFor('add') !== null}
								title={reasonFor('add') ?? 'Add a block to the canvas'}
								onclick={openAddAtCenter}
								data-testid="add-block-phone"
							>
								<PlusIcon /> Add block
							</Button>
						</div>
					{/if}

					{#if issues.length > 0 && !issuesHidden}
						<div
							class="absolute top-2 left-2 z-10 flex w-72 max-w-full flex-col items-start gap-1.5"
							data-testid="problems-panel"
						>
							<div
								class="flex items-center gap-1 rounded-md border border-warning-border bg-warning-surface py-1 pr-1 pl-2 text-xs shadow-sm"
							>
								<button
									type="button"
									class="flex items-center gap-1 rounded-sm py-0.5 font-semibold focus-visible:outline-2 focus-visible:outline-ring"
									aria-expanded={issuesOpen}
									aria-controls="problems-list"
									onclick={() => (issuesOpen = !issuesOpen)}
								>
									{issues.length}
									{issues.length === 1 ? 'thing needs' : 'things need'} your attention
									<ChevronDownIcon
										class={[
											'size-3.5 transition-transform',
											issuesOpen && 'rotate-180'
										]}
									/>
								</button>
								<button
									type="button"
									class="rounded-sm p-1 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
									aria-label="Hide the problems list"
									onclick={() => (issuesHidden = true)}
								>
									<XIcon class="size-3.5" />
								</button>
							</div>
							{#if issuesOpen}
								<div
									id="problems-list"
									class="flex max-h-56 w-full flex-col gap-1 overflow-y-auto rounded-lg border border-warning-border bg-warning-surface p-3 text-xs shadow-md"
								>
									{#each generalIssues as issue (issue.message)}
										<p>{issue.message}</p>
									{/each}
									{#each issues.filter((issue) => issue.node_id) as issue (issue.node_id + issue.message)}
										<button
											type="button"
											class="rounded-sm text-left underline-offset-2 hover:underline focus-visible:outline-2 focus-visible:outline-ring"
											onclick={() => {
												selectedId = issue.node_id ?? null;
												nodes = nodes.map((node) => ({
													...node,
													selected: node.id === issue.node_id
												}));
											}}>{issue.message}</button
										>
									{/each}
								</div>
							{/if}
						</div>
					{/if}
				</div>

				{#if selectedNode}
					<div
						class="w-80 shrink-0 max-md:absolute max-md:inset-x-0 max-md:bottom-0 max-md:z-10 max-md:h-1/2 max-md:w-auto max-md:border-t max-md:border-border"
					>
						{#if selectedNode.type === 'note'}
							<aside
								class="flex h-full flex-col gap-3 border-l border-border bg-card p-3"
							>
								<h2 class="text-sm font-semibold">Note</h2>
								<textarea
									id="note-text"
									class="min-h-32 w-full rounded-md border border-input bg-card p-2 text-sm outline-none focus-visible:outline-2 focus-visible:outline-ring"
									aria-label="Note text"
									value={selectedNode.data.title}
									disabled={readOnly}
									oninput={(event) =>
										setNoteText(selectedNode.id, event.currentTarget.value)}
								></textarea>
								<p class="text-xs text-muted-foreground">
									Notes stay in this browser; they are not part of the published
									automation.
								</p>
								{#if !readOnly}
									<Button
										size="sm"
										variant="outline"
										onclick={() => deleteNodes([selectedNode.id])}
									>
										<Trash2Icon /> Delete note
									</Button>
								{/if}
							</aside>
						{:else}
							<Inspector
								{projectId}
								{workflowId}
								data={selectedNode.data}
								def={defs.get(selectedNode.data.typeId)}
								issues={issuesByNode.get(selectedNode.id) ?? []}
								runs={selectedRuns}
								details={blockDetails}
								{extractionFields}
								{readOnly}
								onconfig={(key, value) => setConfig(selectedNode.id, key, value)}
								onrename={(label) => rename(selectedNode.id, label)}
								ondelete={() => deleteNodes([selectedNode.id])}
								onclose={() => {
									selectedId = null;
									nodes = nodes.map((node) => ({ ...node, selected: false }));
								}}
							/>
						{/if}
					</div>
				{/if}
			{/if}
		</div>
	{/if}
</div>

{#if menu}
	<CanvasMenu
		x={menu.x}
		y={menu.y}
		entries={menu.entries}
		startInAdd={menu.startInAdd}
		{catalog}
		accepts={menu.accepts}
		{hasTrigger}
		presentation={narrow.current ? 'sheet' : 'popover'}
		onpick={pickFromMenu}
		onclose={() => (menu = null)}
	/>
{/if}

{#if hint}
	<div
		role="status"
		class="fixed z-50 max-w-64 rounded-md border border-border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md"
		style:left="{Math.min(
			hint.x + 12,
			(typeof window === 'undefined' ? 0 : window.innerWidth) - 272
		)}px"
		style:top="{hint.y + 12}px"
		data-testid="connection-hint"
	>
		{hint.text}
	</div>
{/if}

<Dialog.Root bind:open={testOpen}>
	<Dialog.Content>
		<Dialog.Header>
			<Dialog.Title>Test this automation</Dialog.Title>
			<Dialog.Description>
				The automation runs now with made-up sample information, starting from “{triggerText}”.
				Steps that would change something — project data, messages, web requests — only
				pretend to run and tell you what they would have done.
			</Dialog.Description>
		</Dialog.Header>
		<div class="max-h-64 overflow-auto rounded-md border border-border bg-muted/40 p-3 text-sm">
			<p class="mb-1 text-3xs tracking-caps text-muted-foreground uppercase">
				Sample information
			</p>
			<DataView value={sampleTriggerData(triggerType)} />
		</div>
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (testOpen = false)}>Cancel</Button>
			<Button onclick={runTest} data-testid="run-test"><FlaskConicalIcon /> Run test</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
