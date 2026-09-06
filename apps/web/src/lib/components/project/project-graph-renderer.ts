import type { GraphEdgeDto, GraphNodeDto } from '$lib/api/generated/models';
import type { GraphOverlayField } from './context.svelte.js';
import { appraisalStatus, provenanceStatus, screeningStatus, studyStatus } from './graph-overlays';
import { getGraphNodeSize } from './graph-layout';
import { reportLabel } from './report-label';
import type SigmaType from 'sigma';
import type { NodeHoverDrawingFunction, NodeLabelDrawingFunction } from 'sigma/rendering';
import type { Settings } from 'sigma/settings';
import type { MouseCoords } from 'sigma/types';

export type ProjectGraphRenderModel = {
	nodes: GraphNodeDto[];
	edges: GraphEdgeDto[];
	visibleNodeIds: Set<string>;
	selectedArticle: string | undefined;
	colorBy: GraphOverlayField;
	fields: readonly GraphOverlayField[];
};

type ProjectGraphRendererCallbacks = {
	onSelect: (reportId: string) => void;
	onClear: () => void;
};

export type ProjectGraphRenderer = {
	mount: (target: HTMLDivElement) => void;
	update: (model: ProjectGraphRenderModel, options?: { resetCamera?: boolean }) => Promise<void>;
	setVisibleNodes: (visibleNodeIds: Set<string>) => void;
	setSelection: (selectedArticle: string | undefined) => void;
	refreshAppearance: (appearance: Pick<ProjectGraphRenderModel, 'colorBy' | 'fields'>) => void;
	reset: () => Promise<void>;
	clear: () => void;
	destroy: () => void;
};

export function createProjectGraphRenderer(
	callbacks: ProjectGraphRendererCallbacks
): ProjectGraphRenderer {
	let target: HTMLDivElement | undefined;
	let renderer: SigmaType<GraphNodeAttributes, GraphEdgeAttributes> | undefined;
	let graph:
		ReturnType<SigmaType<GraphNodeAttributes, GraphEdgeAttributes>['getGraph']> | undefined;
	let themeObserver: MutationObserver | undefined;
	let renderRun = 0;
	let lastDragEndedAt = 0;
	let model: ProjectGraphRenderModel = {
		nodes: [],
		edges: [],
		visibleNodeIds: new Set(),
		selectedArticle: undefined,
		colorBy: 'metrics',
		fields: []
	};
	const interaction: GraphInteractionState = {
		visibleNodeIds: new Set(),
		selectedArticle: undefined,
		selectedNode: undefined,
		hoveredNode: undefined,
		focusedNodeIds: new Set(),
		focusedEdgeIds: new Set(),
		draggedNode: undefined,
		dragMoved: false,
		palette: {
			background: '#f8fafc',
			nodeDefault: '#64748b',
			nodeCited: '#2563eb',
			nodeSelected: '#d97706',
			nodeFocused: '#0891b2',
			nodeDimmed: '#cbd5e1',
			edgeDefault: '#94a3b8',
			edgeFocused: '#0284c7',
			edgeDimmed: '#dbe4ee',
			label: '#0f172a',
			labelMuted: '#475569',
			labelHalo: '#f8fafc',
			hoverBackground: '#ffffff'
		}
	};

	function overlayEnabled(field: GraphOverlayField): boolean {
		return model.fields.includes(field);
	}

	function internalCitations(node: GraphNodeDto): number {
		return node.metrics?.internal_citations ?? 0;
	}

	const DIMMED_NODE_ALPHA = 0.22;
	const DIMMED_EDGE_ALPHA = 0.24;
	const NORMAL_LABEL_MAX_WIDTH = 220;
	const HOVER_LABEL_MAX_WIDTH = 360;
	const SMALL_GRAPH_NODE_COUNT = 100;
	const LARGE_GRAPH_NODE_COUNT = 500;
	const SMALL_GRAPH_FORCE_ATLAS_ITERATIONS = 120;
	const MEDIUM_GRAPH_FORCE_ATLAS_ITERATIONS = 220;
	const LARGE_GRAPH_FORCE_ATLAS_ITERATIONS = 300;
	const SMALL_GRAPH_NOVERLAP_ITERATIONS = 150;
	const MEDIUM_GRAPH_NOVERLAP_ITERATIONS = 300;
	const LARGE_GRAPH_NOVERLAP_ITERATIONS = 500;
	type GraphPalette = {
		background: string;
		nodeDefault: string;
		nodeCited: string;
		nodeSelected: string;
		nodeFocused: string;
		nodeDimmed: string;
		edgeDefault: string;
		edgeFocused: string;
		edgeDimmed: string;
		label: string;
		labelMuted: string;
		labelHalo: string;
		hoverBackground: string;
	};

	type GraphNodeAttributes = GraphNodeDto & {
		label: string;
		fullLabel: string;
		type: string;
		x: number;
		y: number;
		size: number;
		color?: string;
		hidden?: boolean;
		highlighted?: boolean;
		forceLabel?: boolean;
		zIndex?: number;
	};

	type GraphEdgeAttributes = {
		size: number;
		color?: string;
		hidden?: boolean;
		zIndex?: number;
	};
	type GraphInstance = ReturnType<
		SigmaType<GraphNodeAttributes, GraphEdgeAttributes>['getGraph']
	>;
	type GraphConstructor = (typeof import('graphology'))['default'];
	type SigmaConstructor = (typeof import('sigma'))['default'];
	type ForceAtlas2Module = typeof import('graphology-layout-forceatlas2');
	type NoverlapModule = typeof import('graphology-layout-noverlap');

	type GraphLabelData = {
		color?: string;
		doi?: string;
		report_id?: string;
		fullLabel?: string | null;
		label?: string | null;
		size: number;
		x: number;
		y: number;
	};

	type GraphInteractionState = {
		visibleNodeIds: Set<string>;
		selectedArticle: string | undefined;
		selectedNode: string | undefined;
		hoveredNode: string | undefined;
		focusedNodeIds: Set<string>;
		focusedEdgeIds: Set<string>;
		draggedNode: string | undefined;
		dragMoved: boolean;
		palette: GraphPalette;
	};

	const fallbackGraphPalette: GraphPalette = {
		background: '#f8fafc',
		nodeDefault: '#64748b',
		nodeCited: '#2563eb',
		nodeSelected: '#d97706',
		nodeFocused: '#0891b2',
		nodeDimmed: '#cbd5e1',
		edgeDefault: '#94a3b8',
		edgeFocused: '#0284c7',
		edgeDimmed: '#dbe4ee',
		label: '#0f172a',
		labelMuted: '#475569',
		labelHalo: '#f8fafc',
		hoverBackground: '#ffffff'
	};
	function readGraphPalette(target: HTMLElement): GraphPalette {
		const source = target.closest('.graph-frame') ?? target;
		const styles = getComputedStyle(source);
		const read = (name: string, fallback: string) =>
			styles.getPropertyValue(name).trim() || fallback;

		return {
			background: read('--graph-bg', fallbackGraphPalette.background),
			nodeDefault: read('--graph-node-default', fallbackGraphPalette.nodeDefault),
			nodeCited: read('--graph-node-cited', fallbackGraphPalette.nodeCited),
			nodeSelected: read('--graph-node-selected', fallbackGraphPalette.nodeSelected),
			nodeFocused: read('--graph-node-focused', fallbackGraphPalette.nodeFocused),
			nodeDimmed: read('--graph-node-dimmed', fallbackGraphPalette.nodeDimmed),
			edgeDefault: read('--graph-edge-default', fallbackGraphPalette.edgeDefault),
			edgeFocused: read('--graph-edge-focused', fallbackGraphPalette.edgeFocused),
			edgeDimmed: read('--graph-edge-dimmed', fallbackGraphPalette.edgeDimmed),
			label: read('--graph-label', fallbackGraphPalette.label),
			labelMuted: read('--graph-label-muted', fallbackGraphPalette.labelMuted),
			labelHalo: read('--graph-label-halo', fallbackGraphPalette.labelHalo),
			hoverBackground: read('--graph-hover-background', fallbackGraphPalette.hoverBackground)
		};
	}

	function updateGraphPalette(target: HTMLElement) {
		interaction.palette = readGraphPalette(target);
		renderer?.setSettings(buildSigmaSettings());
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function setupThemeObserver(target: HTMLElement) {
		themeObserver?.disconnect();
		themeObserver = new MutationObserver(() => updateGraphPalette(target));
		themeObserver.observe(document.documentElement, {
			attributes: true,
			attributeFilter: ['class']
		});
	}

	function withAlpha(color: string, alpha: number) {
		const trimmed = color.trim();
		const hex = trimmed.match(/^#([0-9a-f]{3}|[0-9a-f]{6})$/i)?.[1];
		if (!hex) return trimmed;

		const expanded =
			hex.length === 3
				? hex
						.split('')
						.map((char) => char + char)
						.join('')
				: hex;
		const value = Number.parseInt(expanded, 16);
		const red = (value >> 16) & 255;
		const green = (value >> 8) & 255;
		const blue = value & 255;

		return `rgba(${red}, ${green}, ${blue}, ${alpha})`;
	}

	function waitForNextFrame() {
		return new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
	}

	function getBaseNodeColor(node: GraphNodeDto) {
		if (node.report_id === interaction.selectedArticle) return interaction.palette.nodeSelected;
		const colorBy = model.colorBy;
		if (!overlayEnabled(colorBy)) return interaction.palette.nodeDefault;
		switch (colorBy) {
			case 'screening':
				return screeningNodeColor(node);
			case 'study':
				return studyNodeColor(node);
			case 'appraisal':
				return appraisalNodeColor(node);
			case 'provenance':
				return provenanceNodeColor(node);
			case 'metrics':
				return metricNodeColor(node);
			default: {
				const exhaustive: never = colorBy;
				return exhaustive;
			}
		}
	}

	function screeningNodeColor(node: GraphNodeDto): string {
		const status = screeningStatus(node);
		if (status === 'include') return interaction.palette.nodeCited;
		if (status === 'exclude') return interaction.palette.nodeSelected;
		return status === 'not-loaded'
			? interaction.palette.nodeDefault
			: interaction.palette.nodeFocused;
	}

	function studyNodeColor(node: GraphNodeDto): string {
		const status = studyStatus(node);
		if (status === 'not-loaded') return interaction.palette.nodeDefault;
		return status === 'grouped'
			? interaction.palette.nodeFocused
			: interaction.palette.nodeDimmed;
	}

	function appraisalNodeColor(node: GraphNodeDto): string {
		return appraisalStatus(node) === 'appraised'
			? interaction.palette.nodeCited
			: interaction.palette.nodeDefault;
	}

	function provenanceNodeColor(node: GraphNodeDto): string {
		const status = provenanceStatus(node);
		if (status === 'not-loaded') return interaction.palette.nodeDefault;
		return status === 'acquired'
			? interaction.palette.nodeFocused
			: interaction.palette.nodeDimmed;
	}

	function metricNodeColor(node: GraphNodeDto): string {
		return internalCitations(node) > 0
			? interaction.palette.nodeCited
			: interaction.palette.nodeDefault;
	}

	function reducedNodeAppearance(
		node: string,
		data: GraphNodeAttributes
	): Pick<GraphNodeAttributes, 'color' | 'label' | 'highlighted' | 'forceLabel' | 'zIndex'> {
		const selected = data.report_id === interaction.selectedArticle;
		const hovered = node === interaction.hoveredNode;
		const focused = interaction.focusedNodeIds.has(node);
		const focusActive = Boolean(interaction.hoveredNode || interaction.selectedNode);
		const base = {
			color: getBaseNodeColor(data),
			label: data.label,
			highlighted: hovered || selected,
			forceLabel: false,
			zIndex: selected ? 3 : 0
		};
		if (!focusActive) return base;
		if (hovered) {
			return { ...base, color: interaction.palette.nodeFocused, forceLabel: true, zIndex: 4 };
		}
		if (focused) {
			return {
				...base,
				color: selected
					? interaction.palette.nodeSelected
					: interaction.palette.nodeFocused,
				forceLabel: true,
				zIndex: Math.max(base.zIndex, 2)
			};
		}
		return {
			...base,
			color: withAlpha(interaction.palette.nodeDimmed, DIMMED_NODE_ALPHA),
			label: ''
		};
	}

	function createNodeReducer() {
		return (node: string, data: GraphNodeAttributes): GraphNodeAttributes => {
			if (!interaction.visibleNodeIds.has(node)) {
				return { ...data, hidden: true };
			}

			return {
				...data,
				...reducedNodeAppearance(node, data),
				hidden: false
			};
		};
	}

	function createEdgeReducer() {
		return reduceEdge;
	}

	function edgeEndpoints(
		edge: string,
		data: GraphEdgeAttributes & { source?: string; target?: string }
	): [string, string] | undefined {
		const extremities = graph?.extremities(edge);
		const source = extremities?.[0] ?? data.source;
		const target = extremities?.[1] ?? data.target;
		return source && target ? [source, target] : undefined;
	}

	function focusedEdgeAppearance(edge: string): GraphEdgeAttributes {
		const focused = interaction.focusedEdgeIds.has(edge);
		return {
			color: focused
				? interaction.palette.edgeFocused
				: withAlpha(interaction.palette.edgeDimmed, DIMMED_EDGE_ALPHA),
			hidden: false,
			size: focused ? 2 : 0.5,
			zIndex: focused ? 2 : 0
		};
	}

	function reduceEdge(
		edge: string,
		data: GraphEdgeAttributes & { source?: string; target?: string }
	): GraphEdgeAttributes {
		const endpoints = edgeEndpoints(edge, data);
		if (!endpoints) return { ...data, hidden: true };
		if (endpoints.some((node) => !interaction.visibleNodeIds.has(node))) {
			return { ...data, hidden: true };
		}
		if (!interaction.hoveredNode && !interaction.selectedNode) {
			return {
				...data,
				color: interaction.palette.edgeDefault,
				hidden: false,
				size: 1,
				zIndex: 0
			};
		}
		return { ...data, ...focusedEdgeAppearance(edge) };
	}

	function buildSigmaSettings(): Partial<Settings<GraphNodeAttributes, GraphEdgeAttributes>> {
		return {
			hideLabelsOnMove: true,
			labelFont: 'IBM Plex Sans Variable, sans-serif',
			labelSize: 12,
			labelWeight: '500',
			labelDensity: 0.45,
			labelGridCellSize: 160,
			labelRenderedSizeThreshold: 8,
			labelColor: { color: interaction.palette.label },
			defaultDrawNodeLabel: drawTruncatedNodeLabel,
			defaultDrawNodeHover: drawNodeHoverLabel,
			nodeReducer: createNodeReducer(),
			edgeReducer: createEdgeReducer(),
			zIndex: true
		};
	}

	function resetGraphCamera() {
		renderer?.getCamera().setState({ x: 0.5, y: 0.5, ratio: 1, angle: 0 });
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function getForceAtlasIterations(nodeCount: number) {
		if (nodeCount >= LARGE_GRAPH_NODE_COUNT) return LARGE_GRAPH_FORCE_ATLAS_ITERATIONS;
		if (nodeCount >= SMALL_GRAPH_NODE_COUNT) return MEDIUM_GRAPH_FORCE_ATLAS_ITERATIONS;
		return SMALL_GRAPH_FORCE_ATLAS_ITERATIONS;
	}

	function getNoverlapIterations(nodeCount: number) {
		if (nodeCount >= LARGE_GRAPH_NODE_COUNT) return LARGE_GRAPH_NOVERLAP_ITERATIONS;
		if (nodeCount >= SMALL_GRAPH_NODE_COUNT) return MEDIUM_GRAPH_NOVERLAP_ITERATIONS;
		return SMALL_GRAPH_NOVERLAP_ITERATIONS;
	}

	function truncateTextToWidth(
		context: CanvasRenderingContext2D,
		text: string,
		maxWidth: number
	) {
		if (context.measureText(text).width <= maxWidth) return text;

		const suffix = '...';
		let low = 0;
		let high = text.length;
		while (low < high) {
			const middle = Math.ceil((low + high) / 2);
			if (context.measureText(text.slice(0, middle) + suffix).width <= maxWidth) {
				low = middle;
			} else {
				high = middle - 1;
			}
		}

		return text.slice(0, low).trimEnd() + suffix;
	}

	function wrapTextToLines(
		context: CanvasRenderingContext2D,
		text: string,
		maxWidth: number,
		maxLines: number
	) {
		const words = text.split(/\s+/).filter(Boolean);
		const lines: string[] = [];
		let current = '';

		for (const word of words) {
			const appended = appendWrappedWord(context, lines, current, word, maxWidth, maxLines);
			current = appended.current;
			if (appended.full) break;
		}
		return finishWrappedLines(context, lines, current, text, maxWidth, maxLines);
	}

	function appendWrappedWord(
		context: CanvasRenderingContext2D,
		lines: string[],
		current: string,
		word: string,
		maxWidth: number,
		maxLines: number
	): { current: string; full: boolean } {
		const next = current ? `${current} ${word}` : word;
		if (context.measureText(next).width <= maxWidth) return { current: next, full: false };
		if (!current) {
			lines.push(truncateTextToWidth(context, word, maxWidth));
			return { current: '', full: lines.length === maxLines };
		}
		lines.push(current);
		return { current: word, full: lines.length === maxLines };
	}

	function finishWrappedLines(
		context: CanvasRenderingContext2D,
		lines: string[],
		current: string,
		text: string,
		maxWidth: number,
		maxLines: number
	): string[] {
		if (lines.length < maxLines && current) lines.push(current);
		if (lines.length === 0) lines.push(truncateTextToWidth(context, text, maxWidth));
		if (lines.length !== maxLines) return lines.slice(0, maxLines);
		const consumed = lines.join(' ');
		if (consumed.length < text.length) {
			lines[maxLines - 1] = truncateTextToWidth(
				context,
				lines[maxLines - 1] + ' ' + text.slice(consumed.length),
				maxWidth
			);
		}
		return lines.slice(0, maxLines);
	}

	function roundedRect(
		context: CanvasRenderingContext2D,
		x: number,
		y: number,
		width: number,
		height: number,
		radius: number
	) {
		const nextRadius = Math.min(radius, width / 2, height / 2);
		context.beginPath();
		context.moveTo(x + nextRadius, y);
		context.lineTo(x + width - nextRadius, y);
		context.quadraticCurveTo(x + width, y, x + width, y + nextRadius);
		context.lineTo(x + width, y + height - nextRadius);
		context.quadraticCurveTo(x + width, y + height, x + width - nextRadius, y + height);
		context.lineTo(x + nextRadius, y + height);
		context.quadraticCurveTo(x, y + height, x, y + height - nextRadius);
		context.lineTo(x, y + nextRadius);
		context.quadraticCurveTo(x, y, x + nextRadius, y);
		context.closePath();
	}

	const drawTruncatedNodeLabel: NodeLabelDrawingFunction<
		GraphNodeAttributes,
		GraphEdgeAttributes
	> = (context, data, settings) => {
		if (!data.label) return;
		const active = Boolean(
			data.report_id === interaction.selectedArticle ||
			data.report_id === interaction.hoveredNode
		);
		if (active) {
			drawFullNodeLabel(context, data, settings);
			return;
		}

		context.save();
		context.font = `${settings.labelWeight} ${settings.labelSize}px ${settings.labelFont}`;
		const text = truncateTextToWidth(context, String(data.label), NORMAL_LABEL_MAX_WIDTH);
		const x = data.x + data.size + 4;
		const y = data.y + settings.labelSize / 3;
		context.lineJoin = 'round';
		context.lineWidth = 4;
		context.strokeStyle = interaction.palette.labelHalo;
		context.fillStyle = interaction.palette.label;
		context.strokeText(text, x, y);
		context.fillText(text, x, y);
		context.restore();
	};

	const drawNodeHoverLabel: NodeHoverDrawingFunction<GraphNodeAttributes, GraphEdgeAttributes> = (
		context,
		data,
		settings
	) => {
		drawFullNodeLabel(context, data, settings);
	};

	function drawFullNodeLabel(
		context: CanvasRenderingContext2D,
		data: GraphLabelData,
		settings: Settings<GraphNodeAttributes, GraphEdgeAttributes>
	) {
		const label = String(data.fullLabel ?? data.label ?? '');
		if (!label) return;

		context.save();
		context.font = `${settings.labelWeight} ${settings.labelSize}px ${settings.labelFont}`;
		const lines = wrapTextToLines(
			context,
			label,
			HOVER_LABEL_MAX_WIDTH,
			Number.POSITIVE_INFINITY
		);
		const lineHeight = settings.labelSize + 4;
		const textWidth = Math.min(
			HOVER_LABEL_MAX_WIDTH,
			Math.max(...lines.map((line) => context.measureText(line).width), 0)
		);
		const paddingX = 8;
		const paddingY = 6;
		const boxWidth = textWidth + paddingX * 2;
		const boxHeight = lines.length * lineHeight + paddingY * 2;
		const boxX = data.x + data.size + 8;
		const boxY = data.y - boxHeight / 2;

		context.beginPath();
		context.arc(data.x, data.y, data.size + 4, 0, Math.PI * 2);
		context.fillStyle = interaction.palette.labelHalo;
		context.fill();
		context.beginPath();
		context.arc(data.x, data.y, data.size + 1, 0, Math.PI * 2);
		context.fillStyle = data.color ?? interaction.palette.nodeFocused;
		context.fill();
		roundedRect(context, boxX, boxY, boxWidth, boxHeight, 6);
		context.fillStyle = interaction.palette.hoverBackground;
		context.fill();
		context.strokeStyle = interaction.palette.edgeFocused;
		context.lineWidth = 1;
		context.stroke();
		context.fillStyle = interaction.palette.label;
		lines.forEach((line, index) => {
			context.fillText(
				line,
				boxX + paddingX,
				boxY + paddingY + settings.labelSize + index * lineHeight
			);
		});
		context.restore();
	}

	function getNodeIdByArticle(reportId: string | undefined) {
		if (!reportId || !graph) return undefined;

		let nodeId: string | undefined;
		graph.someNode((node, attributes) => {
			if (attributes.report_id !== reportId) return false;

			nodeId = node;
			return true;
		});

		return nodeId;
	}

	function updateFocusSets() {
		interaction.focusedNodeIds = new Set();
		interaction.focusedEdgeIds = new Set();
		const focusNode = interaction.hoveredNode ?? interaction.selectedNode;

		if (focusNode && graph?.hasNode(focusNode)) {
			interaction.focusedNodeIds.add(focusNode);
			for (const neighbor of graph.neighbors(focusNode)) {
				interaction.focusedNodeIds.add(neighbor);
			}
			graph.forEachEdge(focusNode, (edge) => {
				interaction.focusedEdgeIds.add(edge);
			});
		}
	}

	function setHoveredNode(node: string | undefined) {
		interaction.hoveredNode = node;
		updateFocusSets();
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function getGraphFrame() {
		return target?.closest('.graph-frame');
	}

	function startNodeDrag(node: string, event: MouseCoords) {
		interaction.draggedNode = node;
		interaction.dragMoved = false;
		setHoveredNode(node);
		event.preventSigmaDefault();
		renderer?.setSetting('enableCameraPanning', false);
		getGraphFrame()?.classList.add('graph-dragging');
	}

	function updateNodeDrag(event: MouseCoords) {
		if (!interaction.draggedNode || !graph || !renderer) return;

		event.preventSigmaDefault();
		event.original.preventDefault();
		event.original.stopPropagation();
		interaction.dragMoved = true;
		const position = renderer.viewportToGraph({ x: event.x, y: event.y });
		graph.mergeNodeAttributes(interaction.draggedNode, {
			x: position.x,
			y: position.y
		});
		renderer.scheduleRefresh({
			partialGraph: { nodes: [interaction.draggedNode] },
			layoutUnchange: true
		});
	}

	function endNodeDrag() {
		const node = interaction.draggedNode;
		if (!node) return;

		if (interaction.dragMoved) {
			lastDragEndedAt = performance.now();
		}
		interaction.draggedNode = undefined;
		interaction.dragMoved = false;
		renderer?.setSetting('enableCameraPanning', true);
		getGraphFrame()?.classList.remove('graph-dragging');
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function updateVisibleNodes(visibleNodeIds: Set<string>) {
		model = { ...model, visibleNodeIds };
		interaction.visibleNodeIds = visibleNodeIds;
		if (interaction.hoveredNode && !visibleNodeIds.has(interaction.hoveredNode)) {
			setHoveredNode(undefined);
		}
		if (interaction.draggedNode && !visibleNodeIds.has(interaction.draggedNode)) {
			endNodeDrag();
		}
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function updateSelection(selectedArticle: string | undefined) {
		model = { ...model, selectedArticle };
		interaction.selectedArticle = selectedArticle;
		interaction.selectedNode = getNodeIdByArticle(selectedArticle);
		updateFocusSets();
		renderer?.scheduleRefresh({ layoutUnchange: true });
	}

	function shouldIgnoreClickAfterDrag() {
		return performance.now() - lastDragEndedAt < 250;
	}

	function registerGraphEvents(nextRenderer: SigmaType) {
		nextRenderer.on('clickNode', ({ node }: { node: string }) => {
			if (interaction.draggedNode || shouldIgnoreClickAfterDrag()) return;
			const article = graph?.getNodeAttributes(node);
			if (article) callbacks.onSelect(article.report_id);
		});
		nextRenderer.on('clickStage', () => {
			if (interaction.draggedNode || shouldIgnoreClickAfterDrag()) return;
			callbacks.onClear();
		});
		nextRenderer.on('enterNode', ({ node }: { node: string }) => {
			if (!interaction.draggedNode) setHoveredNode(node);
		});
		nextRenderer.on('leaveNode', () => {
			if (!interaction.draggedNode) setHoveredNode(undefined);
		});
		nextRenderer.on('leaveStage', () => {
			if (interaction.draggedNode) {
				endNodeDrag();
				return;
			}
			setHoveredNode(undefined);
		});
		nextRenderer.on('downNode', ({ node, event, preventSigmaDefault }) => {
			preventSigmaDefault();
			startNodeDrag(node, event);
		});
		nextRenderer.on('moveBody', ({ event, preventSigmaDefault }) => {
			if (!interaction.draggedNode) return;
			preventSigmaDefault();
			updateNodeDrag(event);
		});
		nextRenderer.on('upStage', endNodeDrag);
		nextRenderer.on('upNode', endNodeDrag);
	}

	function buildGraph(
		Graph: GraphConstructor,
		nodes: GraphNodeDto[],
		edges: GraphEdgeDto[]
	): GraphInstance {
		const nextGraph = new Graph<GraphNodeAttributes, GraphEdgeAttributes>();
		for (const [index, node] of nodes.entries()) {
			nextGraph.addNode(node.report_id, {
				...node,
				label: reportLabel(node),
				fullLabel: reportLabel(node),
				type: 'circle',
				x: Math.cos(index) * 10,
				y: Math.sin(index) * 10,
				size: 4
			});
		}
		for (const edge of edges) addGraphEdge(nextGraph, edge);
		nextGraph.forEachNode((node) => {
			const degree = nextGraph.degree(node);
			nextGraph.setNodeAttribute(node, 'size', getGraphNodeSize(degree, nextGraph.order));
		});
		return nextGraph;
	}

	function addGraphEdge(nextGraph: GraphInstance, edge: GraphEdgeDto): void {
		if (
			!nextGraph.hasNode(edge.source) ||
			!nextGraph.hasNode(edge.target) ||
			edge.source === edge.target ||
			nextGraph.hasEdge(edge.source, edge.target)
		) {
			return;
		}
		nextGraph.addDirectedEdge(edge.source, edge.target, { size: 1 });
	}

	function arrangeGraph(
		nextGraph: GraphInstance,
		forceAtlas2: ForceAtlas2Module,
		noverlap: NoverlapModule
	): void {
		if (nextGraph.order > 2) {
			const inferredSettings = forceAtlas2.default.inferSettings(nextGraph);
			forceAtlas2.default.assign(nextGraph, {
				iterations: getForceAtlasIterations(nextGraph.order),
				settings: {
					...inferredSettings,
					adjustSizes: false,
					barnesHutOptimize: true,
					gravity: inferredSettings.gravity ?? 1,
					scalingRatio: Math.max(inferredSettings.scalingRatio ?? 1, 5),
					slowDown: inferredSettings.slowDown ?? 1
				}
			});
		}
		if (nextGraph.order > 1) {
			noverlap.default.assign(nextGraph, {
				maxIterations: getNoverlapIterations(nextGraph.order),
				settings: { margin: 2, ratio: 1, expansion: 1.1, speed: 3 }
			});
		}
	}

	function installGraph(
		nextGraph: GraphInstance,
		nextTarget: HTMLDivElement,
		Sigma: SigmaConstructor
	): void {
		interaction.visibleNodeIds = model.visibleNodeIds;
		interaction.selectedArticle = model.selectedArticle;
		interaction.palette = readGraphPalette(nextTarget);
		setupThemeObserver(nextTarget);
		if (!renderer) {
			renderer = new Sigma(nextGraph, nextTarget, buildSigmaSettings());
			graph = renderer.getGraph();
			registerGraphEvents(renderer);
			return;
		}
		renderer.setGraph(nextGraph);
		graph = renderer.getGraph();
		renderer.setSettings(buildSigmaSettings());
		renderer.scheduleRefresh({ layoutUnchange: true });
	}

	async function renderGraph(
		target: HTMLDivElement,
		nodes: GraphNodeDto[],
		edges: GraphEdgeDto[],
		options: { resetCamera?: boolean } = {}
	) {
		const run = ++renderRun;

		const [{ default: Graph }, { default: Sigma }, forceAtlas2, noverlap] = await Promise.all([
			import('graphology'),
			import('sigma'),
			import('graphology-layout-forceatlas2'),
			import('graphology-layout-noverlap')
		]);
		if (run !== renderRun) return;

		await waitForNextFrame();
		if (run !== renderRun) return;

		const nextGraph = buildGraph(Graph, nodes, edges);
		arrangeGraph(nextGraph, forceAtlas2, noverlap);
		installGraph(nextGraph, target, Sigma);
		interaction.selectedNode = getNodeIdByArticle(interaction.selectedArticle);
		setHoveredNode(undefined);
		if (options.resetCamera) resetGraphCamera();
	}

	function clearGraph() {
		renderRun += 1;
		endNodeDrag();
		interaction.selectedNode = undefined;
		setHoveredNode(undefined);
		themeObserver?.disconnect();
		themeObserver = undefined;
		renderer?.setSetting('enableCameraPanning', true);
		renderer?.kill();
		renderer = undefined;
		graph = undefined;
	}

	return {
		mount(nextTarget) {
			target = nextTarget;
		},
		async update(nextModel, options = {}) {
			model = nextModel;
			if (!target || model.nodes.length === 0) {
				clearGraph();
				return;
			}
			await renderGraph(target, model.nodes, model.edges, options);
		},
		setVisibleNodes: updateVisibleNodes,
		setSelection: updateSelection,
		refreshAppearance(appearance) {
			model = { ...model, ...appearance };
			renderer?.scheduleRefresh({ layoutUnchange: true });
		},
		async reset() {
			if (!target || model.nodes.length === 0) return;
			await renderGraph(target, model.nodes, model.edges, { resetCamera: true });
		},
		clear: clearGraph,
		destroy() {
			clearGraph();
			target = undefined;
		}
	};
}
