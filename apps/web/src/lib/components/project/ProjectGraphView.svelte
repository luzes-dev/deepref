<script lang="ts">
	import * as Empty from '$lib/components/ui/empty';
	import * as InputGroup from '$lib/components/ui/input-group';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Slider } from '$lib/components/ui/slider';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import { Spinner } from '$lib/components/ui/spinner';
	import GraphDegradedState from '$lib/components/GraphDegradedState.svelte';
	import type { GraphEdgeDto, GraphNodeDto, ProjectGraphDto } from '$lib/api/generated/models';
	import { createGetProjectGraph } from '$lib/api/generated/reports/reports';
	import { createGetProjectProjection } from '$lib/api/generated/projection/projection';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { onCleanup, watch } from 'runed';
	import { untrack } from 'svelte';
	import { useProjectWorkspaceContext, type GraphOverlayField } from './context.svelte.js';
	import {
		appraisalStatus,
		overlaySummary,
		provenanceStatus,
		screeningStatus,
		studyStatus
	} from './graph-overlays';
	import { getGraphNodeSize } from './graph-layout';
	import { reportLabel, reportSearchText } from './report-label';
	import type SigmaType from 'sigma';
	import type { NodeHoverDrawingFunction, NodeLabelDrawingFunction } from 'sigma/rendering';
	import type { Settings } from 'sigma/settings';
	import type { MouseCoords } from 'sigma/types';

	const FILTER_UPDATE_DELAY_MS = 150;
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
	const overlayLabels: ReadonlyArray<{ field: GraphOverlayField; label: string }> = [
		{ field: 'metrics', label: 'Metrics' },
		{ field: 'screening', label: 'Screening' },
		{ field: 'study', label: 'Study' },
		{ field: 'appraisal', label: 'Appraisal' },
		{ field: 'provenance', label: 'Provenance' }
	];

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

	const workspace = useProjectWorkspaceContext();
	let container = $state<HTMLDivElement | null>(null);
	let renderer: SigmaType<GraphNodeAttributes, GraphEdgeAttributes> | undefined;
	let graph:
		ReturnType<SigmaType<GraphNodeAttributes, GraphEdgeAttributes>['getGraph']> | undefined;
	let themeObserver: MutationObserver | undefined;
	let renderRun = 0;
	let graphRendering = $state(false);
	const enabled = $derived(workspace.view === 'graph');
	const interaction: GraphInteractionState = {
		visibleNodeIds: new Set(),
		selectedArticle: undefined,
		selectedNode: undefined,
		hoveredNode: undefined,
		focusedNodeIds: new Set(),
		focusedEdgeIds: new Set(),
		draggedNode: undefined,
		dragMoved: false,
		palette: fallbackGraphPalette
	};
	let lastDragEndedAt = 0;

	const graphQuery = createGetProjectGraph(
		() => workspace.project.id,
		() => ({ fields: workspace.graphFilters.fields.join(',') }),
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
	);
	const projectionQuery = createGetProjectProjection(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
	);
	const graphData = $derived<ProjectGraphDto>(
		graphQuery.data?.data ?? {
			nodes: [],
			edges: [],
			projection: { revision: 0, lag: 0 },
			truncated: false
		}
	);
	function internalCitations(node: GraphNodeDto): number {
		return node.metrics?.internal_citations ?? 0;
	}

	function overlayEnabled(field: GraphOverlayField): boolean {
		return workspace.graphFilters.fields.includes(field);
	}

	const visibleNodes = $derived(
		graphData.nodes.filter((node) => {
			const term = workspace.graphFilters.search.toLowerCase();
			return (
				(!overlayEnabled('metrics') ||
					internalCitations(node) >= workspace.graphFilters.minInternal) &&
				reportSearchText(node).includes(term)
			);
		})
	);
	const selectedGraphNode = $derived(
		graphData.nodes.find((node) => node.report_id === workspace.selectedArticle)
	);

	function getVisibleNodeIds() {
		return new Set(visibleNodes.map((node) => node.report_id));
	}

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
		const colorBy = workspace.graphFilters.colorBy;
		if (!overlayEnabled(colorBy)) return interaction.palette.nodeDefault;
		if (colorBy === 'screening') {
			const status = screeningStatus(node);
			if (status === 'not-loaded') return interaction.palette.nodeDefault;
			if (status === 'include') return interaction.palette.nodeCited;
			if (status === 'exclude') return interaction.palette.nodeSelected;
			return interaction.palette.nodeFocused;
		}
		if (colorBy === 'study') {
			if (studyStatus(node) === 'not-loaded') return interaction.palette.nodeDefault;
			return studyStatus(node) === 'grouped'
				? interaction.palette.nodeFocused
				: interaction.palette.nodeDimmed;
		}
		if (colorBy === 'appraisal') {
			if (appraisalStatus(node) === 'not-loaded') return interaction.palette.nodeDefault;
			return appraisalStatus(node) === 'appraised'
				? interaction.palette.nodeCited
				: interaction.palette.nodeDefault;
		}
		if (colorBy === 'provenance') {
			if (provenanceStatus(node) === 'not-loaded') return interaction.palette.nodeDefault;
			return provenanceStatus(node) === 'acquired'
				? interaction.palette.nodeFocused
				: interaction.palette.nodeDimmed;
		}
		if (internalCitations(node) > 0) return interaction.palette.nodeCited;
		return interaction.palette.nodeDefault;
	}

	function createNodeReducer() {
		return (node: string, data: GraphNodeAttributes): GraphNodeAttributes => {
			if (!interaction.visibleNodeIds.has(node)) {
				return { ...data, hidden: true };
			}

			const selected = data.report_id === interaction.selectedArticle;
			const hovered = node === interaction.hoveredNode;
			const focused = interaction.focusedNodeIds.has(node);
			const focusActive = Boolean(interaction.hoveredNode || interaction.selectedNode);
			let color = getBaseNodeColor(data);
			let label = data.label;
			let forceLabel = false;
			let zIndex = selected ? 3 : 0;

			if (focusActive) {
				if (hovered) {
					color = interaction.palette.nodeFocused;
					forceLabel = true;
					zIndex = 4;
				} else if (focused) {
					color = selected
						? interaction.palette.nodeSelected
						: interaction.palette.nodeFocused;
					forceLabel = true;
					zIndex = Math.max(zIndex, 2);
				} else {
					color = withAlpha(interaction.palette.nodeDimmed, DIMMED_NODE_ALPHA);
					label = '';
				}
			}

			return {
				...data,
				color,
				label,
				hidden: false,
				highlighted: hovered || selected,
				forceLabel,
				zIndex
			};
		};
	}

	function createEdgeReducer() {
		return (
			edge: string,
			data: GraphEdgeAttributes & { source?: string; target?: string }
		): GraphEdgeAttributes => {
			const extremities = graph?.extremities(edge);
			const source = extremities?.[0] ?? data.source;
			const target = extremities?.[1] ?? data.target;
			if (!source || !target) return { ...data, hidden: true };

			if (
				!interaction.visibleNodeIds.has(source) ||
				!interaction.visibleNodeIds.has(target)
			) {
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

			const focused = interaction.focusedEdgeIds.has(edge);
			return {
				...data,
				color: focused
					? interaction.palette.edgeFocused
					: withAlpha(interaction.palette.edgeDimmed, DIMMED_EDGE_ALPHA),
				hidden: false,
				size: focused ? 2 : 0.5,
				zIndex: focused ? 2 : 0
			};
		};
	}

	function buildSigmaSettings(): Partial<Settings<GraphNodeAttributes, GraphEdgeAttributes>> {
		return {
			hideLabelsOnMove: true,
			labelFont: 'Inter Variable, sans-serif',
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
			const next = current ? `${current} ${word}` : word;
			if (context.measureText(next).width <= maxWidth) {
				current = next;
				continue;
			}

			if (!current) {
				lines.push(truncateTextToWidth(context, word, maxWidth));
				if (lines.length === maxLines) break;
				continue;
			}

			if (current) lines.push(current);
			current = word;
			if (lines.length === maxLines) break;
		}

		if (lines.length < maxLines && current) lines.push(current);
		if (lines.length === 0) lines.push(truncateTextToWidth(context, text, maxWidth));

		if (lines.length === maxLines) {
			const consumed = lines.join(' ');
			if (consumed.length < text.length) {
				lines[maxLines - 1] = truncateTextToWidth(
					context,
					lines[maxLines - 1] + ' ' + text.slice(consumed.length),
					maxWidth
				);
			}
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
		return container?.closest('.graph-frame');
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
			const article = graph?.getNodeAttributes(node) as GraphNodeDto | undefined;
			if (article) workspace.openArticle(article.report_id);
		});
		nextRenderer.on('clickStage', () => {
			if (interaction.draggedNode || shouldIgnoreClickAfterDrag()) return;
			workspace.clearArticle();
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

	async function renderGraph(
		target: HTMLDivElement,
		nodes: GraphNodeDto[],
		edges: GraphEdgeDto[],
		options: { resetCamera?: boolean } = {}
	) {
		const run = ++renderRun;
		graphRendering = true;

		try {
			const [{ default: Graph }, { default: Sigma }, forceAtlas2, noverlap] =
				await Promise.all([
					import('graphology'),
					import('sigma'),
					import('graphology-layout-forceatlas2'),
					import('graphology-layout-noverlap')
				]);
			if (run !== renderRun) return;

			await waitForNextFrame();
			if (run !== renderRun) return;

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
			for (const edge of edges) {
				if (
					nextGraph.hasNode(edge.source) &&
					nextGraph.hasNode(edge.target) &&
					edge.source !== edge.target &&
					!nextGraph.hasEdge(edge.source, edge.target)
				) {
					nextGraph.addDirectedEdge(edge.source, edge.target, {
						size: 1
					});
				}
			}
			nextGraph.forEachNode((node) => {
				const degree = nextGraph.degree(node);
				nextGraph.setNodeAttribute(node, 'size', getGraphNodeSize(degree, nextGraph.order));
			});
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
			graph = nextGraph;
			if (nextGraph.order > 1) {
				noverlap.default.assign(nextGraph, {
					maxIterations: getNoverlapIterations(nextGraph.order),
					settings: {
						margin: 2,
						ratio: 1,
						expansion: 1.1,
						speed: 3
					}
				});
			}
			interaction.visibleNodeIds = untrack(getVisibleNodeIds);
			interaction.selectedArticle = untrack(() => workspace.selectedArticle);
			interaction.palette = readGraphPalette(target);
			setupThemeObserver(target);
			if (renderer) {
				renderer.setGraph(nextGraph);
				graph = renderer.getGraph();
				renderer.setSettings(buildSigmaSettings());
				renderer.scheduleRefresh({ layoutUnchange: true });
			} else {
				renderer = new Sigma(nextGraph, target, buildSigmaSettings());
				graph = renderer.getGraph();
				registerGraphEvents(renderer);
			}
			interaction.selectedNode = getNodeIdByArticle(interaction.selectedArticle);
			setHoveredNode(undefined);
			if (options.resetCamera) resetGraphCamera();
		} finally {
			if (run === renderRun) graphRendering = false;
		}
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
		graphRendering = false;
	}

	function resetLayout() {
		if (container && enabled && visibleNodes.length > 0) {
			void renderGraph(container, graphData.nodes, graphData.edges, {
				resetCamera: true
			});
		}
	}

	function graphContainerAttachment(element: Element) {
		container = element as HTMLDivElement;
		return () => {
			if (container === element) container = null;
		};
	}

	const visibleNodeIds = $derived(getVisibleNodeIds());

	watch(
		[() => enabled, () => container, () => graphData.nodes, () => graphData.edges],
		([active, target, nodes, edges]) => {
			if (!active || !target || nodes.length === 0) {
				clearGraph();
				return;
			}

			void renderGraph(target, nodes, edges);
		}
	);

	watch([() => enabled, () => visibleNodeIds], ([active, nextVisibleNodeIds]) => {
		if (!active) return;

		const timeout = setTimeout(
			() => updateVisibleNodes(nextVisibleNodeIds),
			FILTER_UPDATE_DELAY_MS
		);
		return () => clearTimeout(timeout);
	});

	watch([() => enabled, () => workspace.selectedArticle], ([active, selectedArticle]) => {
		if (!active) return;

		updateSelection(selectedArticle);
	});

	watch(
		[
			() => enabled,
			() => workspace.graphFilters.colorBy,
			() => workspace.graphFilters.fields.join(',')
		],
		([active]) => {
			if (!active) return;
			renderer?.scheduleRefresh({ layoutUnchange: true });
		}
	);

	onCleanup(() => clearGraph());
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">Graph</h1>
			<p class="text-sm text-muted-foreground">
				Project-local citation network, directed from citing work to cited work.
			</p>
		</div>
		{#if graphQuery.data}
			<div class="flex flex-wrap gap-2" data-testid="projection-metadata">
				<Badge variant="secondary"
					>Projection revision {graphData.projection.revision}</Badge
				>
				<Badge variant="outline">Lag {graphData.projection.lag}</Badge>
				{#if graphData.projection.last_success_at}
					<Badge variant="outline">
						Projected {new Date(graphData.projection.last_success_at).toLocaleString()}
					</Badge>
				{/if}
				{#if graphData.truncated}<Badge variant="secondary">Bounded result</Badge>{/if}
			</div>
		{/if}
	</div>

	<div class="grid gap-3 md:grid-cols-[1fr_280px_auto]">
		<InputGroup.Root>
			<InputGroup.Input
				placeholder="Search node label"
				bind:value={workspace.graphFilters.search}
			/>
			<InputGroup.Addon><SearchIcon /></InputGroup.Addon>
		</InputGroup.Root>
		<div class="flex items-center gap-3">
			<Slider
				type="single"
				bind:value={workspace.graphFilters.minInternal}
				max={20}
				step={1}
				disabled={!overlayEnabled('metrics')}
			/>
			<Badge variant="outline">
				{overlayEnabled('metrics')
					? `Internal ${workspace.graphFilters.minInternal}+`
					: 'Internal filter unavailable'}
			</Badge>
		</div>
		<Badge variant="secondary"
			>{graphData.nodes.length} nodes and {graphData.edges.length} edges</Badge
		>
	</div>
	<fieldset class="flex flex-wrap items-center gap-3" data-testid="graph-overlay-filters">
		<legend class="mr-1 text-sm font-medium">Visual overlays</legend>
		{#each overlayLabels as { field, label } (field)}
			<label class="flex items-center gap-2 text-sm text-muted-foreground">
				<Checkbox
					checked={overlayEnabled(field)}
					onCheckedChange={(checked) =>
						workspace.graphFilters.setField(field, checked === true)}
					aria-label={`Load ${label.toLowerCase()} overlay`}
					data-testid={`graph-overlay-${field}`}
				/>
				{label}
			</label>
		{/each}
		<label class="ml-auto flex items-center gap-2 text-sm">
			<span class="text-muted-foreground">Color by</span>
			<select
				aria-label="Color graph by"
				class="h-8 rounded-md border bg-background px-2"
				value={workspace.graphFilters.colorBy}
				onchange={(event) =>
					(workspace.graphFilters.colorBy = (event.currentTarget as HTMLSelectElement)
						.value as GraphOverlayField)}
			>
				{#each overlayLabels as { field, label } (field)}
					<option value={field} disabled={!overlayEnabled(field)}>{label}</option>
				{/each}
			</select>
		</label>
	</fieldset>
	<div class="grid gap-3 md:grid-cols-2" data-testid="graph-overlay-legend">
		{#if !overlayEnabled(workspace.graphFilters.colorBy)}
			<div class="rounded-md border p-3 text-sm text-muted-foreground">
				Color-by overlay is not loaded. Select it above or enable its field.
			</div>
		{:else if workspace.graphFilters.colorBy === 'screening'}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Screening overlay</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					<span class="screening-include">● include</span>
					<span class="screening-exclude">● exclude</span>
					<span class="screening-pending">● pending / unscreened</span>
				</div>
			</div>
		{:else if workspace.graphFilters.colorBy === 'metrics'}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Metrics overlay</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					<span class="metrics-cited">● internally cited</span>
					<span class="metrics-uncited">● no internal citations</span>
				</div>
				<p class="mt-2 text-xs text-muted-foreground">
					Rank and citation counts are available when a node is selected.
				</p>
			</div>
		{:else}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Evidence overlays</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					{#if workspace.graphFilters.colorBy === 'study'}<span class="evidence-grouped"
							>● grouped</span
						><span class="evidence-ungrouped">● ungrouped</span>{/if}
					{#if workspace.graphFilters.colorBy === 'appraisal'}<span
							class="evidence-appraised">● appraised</span
						><span class="evidence-not-appraised">● not appraised</span>{/if}
					{#if workspace.graphFilters.colorBy === 'provenance'}<span
							class="evidence-acquired">● acquired</span
						><span class="evidence-no-source">● no source</span>{/if}
				</div>
			</div>
		{/if}
	</div>
	{#if selectedGraphNode}
		<aside class="rounded-md border bg-muted/20 p-3" aria-label="Selected node overlay summary">
			<div class="text-sm font-medium">Selected node overlays</div>
			<div class="mt-2 grid gap-1 text-sm text-muted-foreground sm:grid-cols-2">
				{#each overlaySummary(selectedGraphNode, workspace.graphFilters.fields) as summary (summary)}
					<div>{summary}</div>
				{/each}
			</div>
		</aside>
	{/if}
	{#if graphQuery.error}
		<GraphDegradedState
			error={graphQuery.error}
			projection={projectionQuery.data?.data}
			onRetry={() => void graphQuery.refetch()}
		/>
	{:else if graphQuery.isPending && enabled}
		<Skeleton class="min-h-[520px] flex-1" />
	{:else if graphData.nodes.length === 0 || visibleNodes.length === 0}
		<Empty.Root class="min-h-[520px] flex-1">
			<Empty.Header>
				<Empty.Title>Empty graph</Empty.Title>
				<Empty.Description
					>No graph nodes match this project and filter set.</Empty.Description
				>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<div class="min-h-0 flex-1">
			<div
				class="graph-frame relative h-full overflow-hidden rounded-md border"
				aria-busy={graphRendering}
			>
				<div {@attach graphContainerAttachment} class="absolute inset-0"></div>
				{#if graphRendering}
					<div
						class="absolute inset-0 z-20 flex items-center justify-center bg-background/70 backdrop-blur-[1px]"
					>
						<div
							class="flex items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm shadow-sm"
						>
							<Spinner class="size-4" />
							<span>Creating graph layout</span>
						</div>
					</div>
				{/if}
				<Button
					variant="ghost"
					size="icon"
					class="absolute top-3 right-3 z-10 bg-background/80"
					onclick={resetLayout}
					disabled={graphRendering}
					aria-label="Reset graph layout"
				>
					<RotateCcwIcon data-icon />
				</Button>
			</div>
		</div>
	{/if}
</div>

<style>
	.graph-frame {
		--graph-bg: #f8fafc;
		--graph-node-default: #64748b;
		--graph-node-cited: #2563eb;
		--graph-node-selected: #d97706;
		--graph-node-focused: #0891b2;
		--graph-node-dimmed: #cbd5e1;
		--graph-edge-default: #94a3b8;
		--graph-edge-focused: #0284c7;
		--graph-edge-dimmed: #dbe4ee;
		--graph-label: #0f172a;
		--graph-label-muted: #475569;
		--graph-label-halo: #f8fafc;
		--graph-hover-background: #ffffff;
		background: var(--graph-bg);
		cursor: grab;
	}

	:global(.dark) .graph-frame {
		--graph-bg: #020617;
		--graph-node-default: #94a3b8;
		--graph-node-cited: #60a5fa;
		--graph-node-selected: #f59e0b;
		--graph-node-focused: #22d3ee;
		--graph-node-dimmed: #334155;
		--graph-edge-default: #475569;
		--graph-edge-focused: #38bdf8;
		--graph-edge-dimmed: #1e293b;
		--graph-label: #e2e8f0;
		--graph-label-muted: #94a3b8;
		--graph-label-halo: #020617;
		--graph-hover-background: #0f172a;
	}

	.graph-frame:global(.graph-dragging) {
		cursor: grabbing;
	}

	.graph-frame :global(canvas) {
		cursor: inherit;
	}

	.screening-include,
	.evidence-appraised {
		color: #2563eb;
	}

	.screening-exclude {
		color: #d97706;
	}

	.screening-pending {
		color: #0891b2;
	}

	.metrics-cited {
		color: #2563eb;
	}

	.metrics-uncited {
		color: #64748b;
	}

	.evidence-grouped,
	.evidence-acquired {
		color: #0891b2;
	}

	.evidence-ungrouped,
	.evidence-no-source {
		color: #cbd5e1;
	}

	.evidence-not-appraised {
		color: #64748b;
	}
</style>
