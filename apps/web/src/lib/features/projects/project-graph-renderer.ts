import type { GraphEdgeDto, GraphNodeDto } from '#lib/api/generated/models/index.js';
import type { GraphOverlayField } from './context.svelte.js';
import { appraisalStatus, provenanceStatus, screeningStatus, studyStatus } from './graph-overlays';
import { fitBBoxWithLabels, getGraphNodeSize, layoutIsolatedNodes } from './graph-layout';
import { reportLabel } from './report-label';
import type SigmaType from 'sigma';
import type { LabelAttachmentContext, LabelAttachmentContent } from 'sigma/primitives';
import type { Settings } from 'sigma/settings';
import type { EdgeDisplayData, MouseCoords, NodeDisplayData, SigmaOptions } from 'sigma/types';

export type ProjectGraphRenderModel = {
	nodes: GraphNodeDto[];
	edges: GraphEdgeDto[];
	visibleNodeIds: Set<string>;
	selectedArticle: string | undefined;
	colorBy: GraphOverlayField;
	fields: readonly GraphOverlayField[];
};

export type ProjectGraphRendererCallbacks = {
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
	let renderer: SigmaRenderer | undefined;
	let graph: GraphInstance | undefined;
	let themeObserver: MutationObserver | undefined;
	let resizeObserver: ResizeObserver | undefined;
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
	const NORMAL_LABEL_MAX_WIDTH = 180;
	const STAGE_PADDING = 24;
	const NODE_FIT_PADDING = 16;
	// Height of the search/layers toolbar floating over the canvas.
	const TOOLBAR_INSET = 56;
	const LABEL_ALL_NODE_COUNT = 150;
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
		// Raw graph attributes double as display data, so they remain indexable.
		[key: string]: unknown;
		label: string;
		fullLabel: string;
		type: string;
		x: number;
		y: number;
		size: number;
	};

	type GraphEdgeAttributes = {
		[key: string]: unknown;
		size: number;
		color?: string;
		zIndex?: number;
	};

	// Sigma 4 keeps the default `Sigma<Attributes, ...>` type instantiations.
	// Renderer-bound declarations carry the project graph generics and only
	// downgrade to raw records where sigma hands them over (reducer calls).
	type SigmaRenderer = SigmaType<GraphNodeAttributes, GraphEdgeAttributes>;
	type ProjectSigmaOptions = SigmaOptions<GraphNodeAttributes, GraphEdgeAttributes>;
	type GraphInstance = ReturnType<
		SigmaType<GraphNodeAttributes, GraphEdgeAttributes>['getGraph']
	>;
	type GraphConstructor = (typeof import('graphology'))['default'];
	type SigmaConstructor = (typeof import('sigma'))['default'];
	type ForceAtlas2Module = typeof import('graphology-layout-forceatlas2');
	type NoverlapModule = typeof import('graphology-layout-noverlap');

	const HOVER_CARD_ATTACHMENT = 'hover-card';
	const LABEL_FONT_SIZE = 12;
	const LABEL_FONT = `500 ${LABEL_FONT_SIZE}px IBM Plex Sans Variable, sans-serif`;
	const LABEL_FONT_FAMILY = 'IBM Plex Sans Variable, sans-serif';
	// Sigma derives the label font from this string via its own font parser,
	// which supports a leading numeric weight.
	const LABEL_FONT_TOKEN = `500 ${LABEL_FONT_FAMILY}`;
	const LABEL_LINE_HEIGHT = LABEL_FONT_SIZE + 4;
	const HOVER_CARD_PADDING_X = 8;
	const HOVER_CARD_PADDING_Y = 6;

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
	// Sigma 4 still parses hex and comma-separated RGB, but not CSS OKLCH, so
	// theme colors are resolved through the browser at this rendering boundary.
	let measureContext: CanvasRenderingContext2D | null | undefined;

	function getMeasureContext(): CanvasRenderingContext2D | null | undefined {
		if (measureContext !== undefined) return measureContext;
		measureContext = document.createElement('canvas').getContext('2d');
		return measureContext;
	}

	function openLabelMeasuring(): CanvasRenderingContext2D | null {
		const context = getMeasureContext();
		if (!context) return null;
		context.font = LABEL_FONT;
		return context;
	}

	function readGraphPalette(target: HTMLElement): GraphPalette {
		const source = target.closest('.graph-frame') ?? target;
		const styles = getComputedStyle(source);
		const canvas = document.createElement('canvas');
		canvas.width = canvas.height = 1;
		const context = canvas.getContext('2d', { willReadFrequently: true });
		const read = (name: string, fallback: string) => {
			if (!context) return fallback;
			context.clearRect(0, 0, 1, 1);
			context.fillStyle = fallback;
			context.fillStyle = styles.getPropertyValue(name).trim() || fallback;
			context.fillRect(0, 0, 1, 1);
			const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
			return `#${[red, green, blue].map((value) => value.toString(16).padStart(2, '0')).join('')}`;
		};

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
		// Reducers read the palette on every refresh, so a scheduled refresh is
		// all it takes to propagate a theme change.
		renderer?.refresh({ schedule: true });
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

	// The native backdrop is used as the hover card frame; the attachment itself
	// carries the wrapped full label. Enabling the backdrop also routes the node
	// through sigma's attachment pipeline.
	function hoverCardAppearance(): Partial<NodeDisplayData> {
		return {
			labelAttachment: HOVER_CARD_ATTACHMENT,
			labelAttachmentPlacement: 'right',
			backdropArea: 'label',
			backdropVisibility: 'visible',
			backdropColor: interaction.palette.hoverBackground,
			backdropBorderColor: interaction.palette.edgeFocused,
			backdropBorderWidth: 1,
			backdropCornerRadius: 6,
			backdropPadding: HOVER_CARD_PADDING_X,
			backdropShadowBlur: 0,
			backdropShadowColor: 'rgba(0, 0, 0, 0)'
		};
	}

	function reducedNodeAppearance(
		node: string,
		data: NodeDisplayData,
		attrs: GraphNodeAttributes
	): Partial<NodeDisplayData> {
		const selected = attrs.report_id === interaction.selectedArticle;
		const hovered = node === interaction.hoveredNode;
		const focused = interaction.focusedNodeIds.has(node);
		const focusActive = Boolean(interaction.hoveredNode || interaction.selectedNode);
		const base: Partial<NodeDisplayData> = {
			color: getBaseNodeColor(attrs),
			label: data.label ?? attrs.label,
			labelColor: interaction.palette.label,
			labelSize: LABEL_FONT_SIZE,
			labelFont: LABEL_FONT_TOKEN,
			labelVisibility: graph && graph.order <= LABEL_ALL_NODE_COUNT ? 'visible' : 'auto',
			visibility: 'visible',
			zIndex: selected ? 3 : 0
		};
		if (!focusActive) return base;
		if (hovered) {
			return {
				...base,
				color: interaction.palette.nodeFocused,
				labelVisibility: 'visible',
				zIndex: 4,
				...hoverCardAppearance()
			};
		}
		if (focused) {
			return {
				...base,
				color: selected
					? interaction.palette.nodeSelected
					: interaction.palette.nodeFocused,
				labelVisibility: 'visible',
				zIndex: Math.max(base.zIndex ?? 0, 2),
				...(selected ? hoverCardAppearance() : {})
			};
		}
		return {
			...base,
			color: withAlpha(interaction.palette.nodeDimmed, DIMMED_NODE_ALPHA),
			labelVisibility: 'hidden'
		};
	}

	function reduceNode(
		node: string,
		data: NodeDisplayData,
		attrs: GraphNodeAttributes
	): Partial<NodeDisplayData> {
		if (!interaction.visibleNodeIds.has(node)) {
			return { visibility: 'hidden' };
		}

		return reducedNodeAppearance(node, data, attrs);
	}

	function reduceEdge(
		edge: string,
		data: EdgeDisplayData,
		attrs: GraphEdgeAttributes & { source?: string; target?: string }
	): Partial<EdgeDisplayData> {
		const endpoints = edgeEndpoints(edge, attrs);
		if (!endpoints || endpoints.some((node) => !interaction.visibleNodeIds.has(node))) {
			return { ...data, visibility: 'hidden', zIndex: 0 };
		}
		if (!interaction.hoveredNode && !interaction.selectedNode) {
			return {
				color: interaction.palette.edgeDefault,
				visibility: 'visible',
				size: 1,
				zIndex: 0
			};
		}
		return { ...focusedEdgeAppearance(edge), visibility: 'visible' };
	}

	function focusedEdgeAppearance(edge: string): Partial<EdgeDisplayData> {
		const focused = interaction.focusedEdgeIds.has(edge);
		return {
			color: focused
				? interaction.palette.edgeFocused
				: withAlpha(interaction.palette.edgeDimmed, DIMMED_EDGE_ALPHA),
			size: focused ? 2 : 0.5,
			zIndex: focused ? 2 : 0
		};
	}

	function edgeEndpoints(
		edge: string,
		data: { source?: string; target?: string }
	): [string, string] | undefined {
		const extremities = graph?.extremities(edge);
		const source = extremities?.[0] ?? data.source;
		const target = extremities?.[1] ?? data.target;
		return source && target ? [source, target] : undefined;
	}

	function buildSigmaSettings(): Partial<Settings> {
		const smallGraph = Boolean(graph && graph.order <= LABEL_ALL_NODE_COUNT);
		return {
			hideLabelsOnMove: true,
			labelDensity: smallGraph ? 4 : 0.45,
			labelGridCellSize: 160,
			labelRenderedSizeThreshold: smallGraph ? 0 : 8,
			stagePadding: STAGE_PADDING,
			// Sigma 4 defaults to "positions", where node sizes grow with zoom; the
			// graph view keeps v3's screen-constant sizing.
			itemSizesReference: 'screen'
		};
	}

	function buildSigmaOptions(): ProjectSigmaOptions {
		return {
			primitives: {
				nodes: {
					label: {
						// The backdrop of a hovered node wraps the label box, the
						// attachment gap and the attachment itself; the margin keeps
						// that frame clear of the node disc.
						margin: 10,
						font: {
							family: LABEL_FONT_FAMILY,
							weight: '500'
						}
					},
					labelAttachments: {
						[HOVER_CARD_ATTACHMENT]: drawHoverCardAttachment
					}
				}
			},
			settings: buildSigmaSettings(),
			nodeReducer: reduceNode,
			edgeReducer: reduceEdge
		};
	}

	function fitGraphToView() {
		if (!renderer || !graph) return;
		const { width, height } = renderer.getDimensions();
		const points: { x: number; y: number }[] = [];
		graph.forEachNode((_, attributes) => points.push({ x: attributes.x, y: attributes.y }));
		const bbox = fitBBoxWithLabels(
			points,
			{ width, height },
			{
				stagePadding: STAGE_PADDING,
				labelWidth: Math.min(NORMAL_LABEL_MAX_WIDTH, Math.max(0, width * 0.35)),
				nodePadding: NODE_FIT_PADDING,
				topInset: TOOLBAR_INSET
			}
		);
		if (bbox) renderer.setCustomBBox(bbox);
	}

	function setupResizeObserver(nextTarget: HTMLDivElement) {
		if (resizeObserver || typeof ResizeObserver === 'undefined') return;
		resizeObserver = new ResizeObserver(() => {
			renderer?.resize();
			fitGraphToView();
		});
		resizeObserver.observe(nextTarget);
	}

	function resetGraphCamera() {
		fitGraphToView();
		renderer?.getCamera().setState({ x: 0.5, y: 0.5, ratio: 1, angle: 0 });
		renderer?.refresh({ schedule: true });
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
		return closeWrappedLines(context, lines, current, text, maxWidth, maxLines);
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

	// Sealed off after the word loop: any line still pending is flushed, an
	// entirely empty wrap means the text itself exceeds the first line width,
	// and once the line cap was reached the leftover text is re-truncated into
	// the final line.
	function closeWrappedLines(
		context: CanvasRenderingContext2D,
		lines: string[],
		current: string,
		text: string,
		maxWidth: number,
		maxLines: number
	): string[] {
		if (lines.length < maxLines && current) lines.push(current);
		if (lines.length === 0) lines.push(truncateTextToWidth(context, text, maxWidth));
		return reflowTruncatedTail(context, lines, text, maxWidth, maxLines);
	}

	function reflowTruncatedTail(
		context: CanvasRenderingContext2D,
		lines: string[],
		text: string,
		maxWidth: number,
		maxLines: number
	): string[] {
		if (lines.length !== maxLines) return lines.slice(0, maxLines);
		const consumed = lines.join(' ');
		if (consumed.length >= text.length) return lines.slice(0, maxLines);
		lines[maxLines - 1] = truncateTextToWidth(
			context,
			`${lines[maxLines - 1]} ${text.slice(consumed.length)}`,
			maxWidth
		);
		return lines.slice(0, maxLines);
	}

	function truncateNodeLabel(label: string): string {
		const context = openLabelMeasuring();
		if (!context) return label;
		return truncateTextToWidth(context, label, NORMAL_LABEL_MAX_WIDTH);
	}

	// Attachment contexts carry the raw graph attributes.
	function readAttachmentLabel(attributes: unknown): string {
		const attrs = attributes as GraphNodeAttributes;
		return String(attrs.fullLabel ?? attrs.label ?? '');
	}

	function buildHoverCardLines(
		label: string
	): { measure: CanvasRenderingContext2D; lines: string[] } | null {
		const measure = openLabelMeasuring();
		if (!measure) return null;
		return { measure, lines: wrapTextToLines(measure, label, HOVER_LABEL_MAX_WIDTH, Infinity) };
	}

	function paintHoverCardLines(
		measure: CanvasRenderingContext2D,
		lines: string[],
		pixelRatio: number
	): HTMLCanvasElement | null {
		const textWidth = Math.min(
			HOVER_LABEL_MAX_WIDTH,
			Math.max(...lines.map((line) => measure.measureText(line).width), 0)
		);
		const boxWidth = textWidth + HOVER_CARD_PADDING_X * 2;
		const boxHeight = lines.length * LABEL_LINE_HEIGHT + HOVER_CARD_PADDING_Y * 2;
		const canvas = document.createElement('canvas');
		canvas.width = Math.max(1, Math.ceil(boxWidth * pixelRatio));
		canvas.height = Math.max(1, Math.ceil(boxHeight * pixelRatio));
		const context = canvas.getContext('2d');
		if (!context) return null;

		paintHoverCardText(context, lines, pixelRatio);
		return canvas;
	}

	function paintHoverCardText(
		context: CanvasRenderingContext2D,
		lines: string[],
		pixelRatio: number
	) {
		context.scale(pixelRatio, pixelRatio);
		context.font = LABEL_FONT;
		context.fillStyle = interaction.palette.label;
		lines.forEach((line, index) => {
			context.fillText(
				line,
				HOVER_CARD_PADDING_X,
				HOVER_CARD_PADDING_Y + LABEL_FONT_SIZE + index * LABEL_LINE_HEIGHT
			);
		});
	}

	function drawHoverCardAttachment({
		attributes,
		pixelRatio
	}: LabelAttachmentContext): LabelAttachmentContent | null {
		const label = readAttachmentLabel(attributes);
		if (!label) return null;
		const prepared = buildHoverCardLines(label);
		if (!prepared) return null;
		const canvas = paintHoverCardLines(prepared.measure, prepared.lines, pixelRatio);
		return canvas ? { type: 'canvas', canvas } : null;
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
		renderer?.refresh({ schedule: true });
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
		// Sigma refreshes itself whenever node attributes change, so no explicit
		// refresh is required here.
		graph.mergeNodeAttributes(interaction.draggedNode, {
			x: position.x,
			y: position.y
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
		renderer?.refresh({ schedule: true });
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
		renderer?.refresh({ schedule: true });
	}

	function updateSelection(selectedArticle: string | undefined) {
		model = { ...model, selectedArticle };
		interaction.selectedArticle = selectedArticle;
		interaction.selectedNode = getNodeIdByArticle(selectedArticle);
		updateFocusSets();
		renderer?.refresh({ schedule: true });
	}

	function shouldIgnoreClickAfterDrag() {
		return performance.now() - lastDragEndedAt < 250;
	}

	function registerGraphEvents(nextRenderer: SigmaRenderer) {
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
			const label = reportLabel(node);
			nextGraph.addNode(node.report_id, {
				...node,
				// Truncation is precomputed because sigma 4 renders labels through
				// its SDF glyph pipeline, which has no canvas-style draw callback.
				label: truncateNodeLabel(label),
				fullLabel: label,
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
		placeIsolatedNodes(nextGraph);
	}

	function placeIsolatedNodes(nextGraph: GraphInstance): void {
		const isolated: string[] = [];
		const connected: { x: number; y: number }[] = [];
		nextGraph.forEachNode((node, attributes) => {
			if (nextGraph.degree(node) === 0) isolated.push(node);
			else connected.push({ x: attributes.x, y: attributes.y });
		});
		if (isolated.length === 0) return;
		const positions = layoutIsolatedNodes(connected, isolated.length);
		isolated.forEach((node, index) => {
			nextGraph.setNodeAttribute(node, 'x', positions[index].x);
			nextGraph.setNodeAttribute(node, 'y', positions[index].y);
		});
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
			// The label settings depend on the graph size, so the graph must be set first.
			renderer = new Sigma(nextGraph, nextTarget, buildSigmaOptions());
			registerGraphEvents(renderer);
			setupResizeObserver(nextTarget);
			fitGraphToView();
			return;
		}
		renderer.setGraph(nextGraph);
		fitGraphToView();
		// setSettings validates, applies and schedules an internal refresh.
		renderer.setSettings(buildSigmaSettings());
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
		graph = nextGraph;
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
		resizeObserver?.disconnect();
		resizeObserver = undefined;
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
			renderer?.refresh({ schedule: true });
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
