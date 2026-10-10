import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GraphEdgeDto, GraphNodeDto } from '#lib/api/generated/models/index.js';
import type { GraphOverlayField } from './context.svelte.js';
import { createProjectGraphRenderer, type ProjectGraphRenderModel } from './project-graph-renderer';

type Attributes = Record<string, unknown>;
type Reducer = (key: string, data: Attributes) => Attributes;
type LabelDrawer = (context: CanvasRenderingContext2D, data: Attributes, settings: object) => void;
type Handler = (payload: Record<string, unknown>) => void;
type FakeGraph = {
	order: number;
	edges: () => string[];
	getNodeAttributes: (node: string) => Attributes;
	getEdgeAttributes: (edge: string) => Attributes;
};

const sigma = vi.hoisted(() => ({ instances: [] as FakeSigma[] }));

class FakeSigma {
	graph: FakeGraph;
	settings: Record<string, unknown>;
	handlers = new Map<string, Handler>();
	scheduleRefresh = vi.fn();
	setCustomBBox = vi.fn();
	setSetting = vi.fn();
	kill = vi.fn();
	camera = { setState: vi.fn() };

	constructor(graph: FakeGraph, _target: HTMLElement, settings: Record<string, unknown>) {
		this.graph = graph;
		this.settings = settings;
		sigma.instances.push(this);
	}
	getGraph() {
		return this.graph;
	}
	setGraph(graph: FakeGraph) {
		this.graph = graph;
	}
	setSettings(settings: Record<string, unknown>) {
		this.settings = { ...this.settings, ...settings };
	}
	getDimensions() {
		return { width: 800, height: 600 };
	}
	getCamera() {
		return this.camera;
	}
	viewportToGraph({ x, y }: { x: number; y: number }) {
		return { x, y };
	}
	on(event: string, handler: Handler) {
		this.handlers.set(event, handler);
	}
	emit(event: string, payload: Record<string, unknown> = {}) {
		this.handlers.get(event)?.(payload);
	}
	node(key: string): Attributes {
		return (this.settings.nodeReducer as Reducer)(key, this.graph.getNodeAttributes(key));
	}
	edge(key: string): Attributes {
		return (this.settings.edgeReducer as Reducer)(key, this.graph.getEdgeAttributes(key));
	}
}

vi.mock('sigma', () => ({ default: FakeSigma }));

const fields: readonly GraphOverlayField[] = [
	'screening',
	'study',
	'appraisal',
	'provenance',
	'metrics'
];

const nodes: GraphNodeDto[] = [
	{
		report_id: 'a',
		title: 'Early mobilisation after cardiac surgery in older adults',
		screening: {
			title_abstract_status: 'include',
			full_text_status: 'include',
			final_status: 'include'
		},
		study: { study_id: 's1', title: 'Study one' },
		appraisal: { assessment_count: 1, completed_count: 1 },
		provenance: { source_record_count: 2, sources: ['pubmed'] },
		metrics: { internal_citations: 3 }
	},
	{
		report_id: 'b',
		title: 'Delirium',
		screening: {
			title_abstract_status: 'exclude',
			full_text_status: 'not_required',
			final_status: 'exclude'
		},
		study: { study_id: null, title: null },
		appraisal: { assessment_count: 1, completed_count: 0 },
		provenance: { source_record_count: 0, sources: [] }
	},
	{
		report_id: 'c',
		title: 'Pending record',
		screening: {
			title_abstract_status: 'pending',
			full_text_status: 'pending',
			final_status: 'pending'
		}
	},
	{ report_id: 'd', title: 'Isolated record' }
] as GraphNodeDto[];

const edges: GraphEdgeDto[] = [
	{ source: 'a', target: 'b' },
	{ source: 'b', target: 'c' },
	{ source: 'a', target: 'b' },
	{ source: 'a', target: 'a' },
	{ source: 'a', target: 'missing' }
] as GraphEdgeDto[];

function model(overrides: Partial<ProjectGraphRenderModel> = {}): ProjectGraphRenderModel {
	return {
		nodes,
		edges,
		visibleNodeIds: new Set(['a', 'b', 'c', 'd']),
		selectedArticle: undefined,
		colorBy: 'metrics',
		fields,
		...overrides
	};
}

function canvasContext(): CanvasRenderingContext2D {
	const noop = () => {};
	return {
		save: noop,
		restore: noop,
		beginPath: noop,
		closePath: noop,
		moveTo: noop,
		lineTo: noop,
		quadraticCurveTo: noop,
		arc: noop,
		fill: noop,
		stroke: noop,
		strokeText: noop,
		fillText: vi.fn(),
		measureText: (text: string) => ({ width: text.length * 7 }) as TextMetrics
	} as unknown as CanvasRenderingContext2D;
}

async function mounted(overrides: Partial<ProjectGraphRenderModel> = {}) {
	const onSelect = vi.fn();
	const onClear = vi.fn();
	const renderer = createProjectGraphRenderer({ onSelect, onClear });
	const target = document.createElement('div');
	document.body.append(target);
	renderer.mount(target);
	await renderer.update(model(overrides));
	const instance = sigma.instances.at(-1);
	if (!instance) throw new Error('sigma was not created');
	return { renderer, instance, onSelect, onClear };
}

beforeEach(() => {
	sigma.instances.length = 0;
	vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
		callback(0);
		return 0;
	});
});

afterEach(() => {
	vi.unstubAllGlobals();
	document.body.innerHTML = '';
});

describe('project graph renderer', () => {
	it('builds a graph without duplicate, self or dangling edges', async () => {
		const { instance } = await mounted();
		expect(instance.graph.order).toBe(4);
		expect(instance.graph.edges()).toHaveLength(2);
		expect(instance.setCustomBBox).toHaveBeenCalled();
		expect(instance.settings.labelDensity).toBe(4);
	});

	it.each(fields)('colors nodes by %s', async (colorBy) => {
		const { instance } = await mounted({ colorBy });
		const colors = ['a', 'b', 'c', 'd'].map((node) => instance.node(node).color);
		expect(colors.every((color) => typeof color === 'string')).toBe(true);
		expect(new Set(colors).size).toBeGreaterThan(1);
	});

	it('uses the default color when the overlay is not loaded', async () => {
		const { instance } = await mounted({ colorBy: 'screening', fields: [] });
		const colors = new Set(['a', 'b', 'c'].map((node) => instance.node(node).color));
		expect(colors.size).toBe(1);
	});

	it('hides nodes and edges outside the visible set', async () => {
		const { renderer, instance } = await mounted();
		renderer.setVisibleNodes(new Set(['a', 'b']));
		expect(instance.node('c').hidden).toBe(true);
		expect(instance.node('a').hidden).toBe(false);
		const [ab, bc] = instance.graph.edges();
		expect(instance.edge(ab!).hidden).toBe(false);
		expect(instance.edge(bc!).hidden).toBe(true);
	});

	it('focuses the hovered node and its neighbours and dims the rest', async () => {
		const { instance } = await mounted();
		instance.emit('enterNode', { node: 'a' });
		expect(instance.node('a')).toMatchObject({ forceLabel: true, zIndex: 4 });
		expect(instance.node('b')).toMatchObject({ forceLabel: true, zIndex: 2 });
		expect(instance.node('d').label).toBe('');
		const [ab, bc] = instance.graph.edges();
		expect(instance.edge(ab!).size).toBe(2);
		expect(instance.edge(bc!).size).toBe(0.5);
		instance.emit('leaveNode');
		expect(instance.node('d').label).not.toBe('');
	});

	it('keeps the selected article highlighted', async () => {
		const { renderer, instance } = await mounted();
		renderer.setSelection('b');
		expect(instance.node('b')).toMatchObject({ highlighted: true, zIndex: 3 });
		expect(instance.node('a').zIndex).toBe(2);
		expect(instance.node('d').label).toBe('');
	});

	it('selects on click and clears on the stage, but not right after a drag', async () => {
		const { instance, onSelect, onClear } = await mounted();
		instance.emit('clickNode', { node: 'a' });
		expect(onSelect).toHaveBeenCalledWith('a');
		instance.emit('clickStage');
		expect(onClear).toHaveBeenCalledTimes(1);

		const original = { preventDefault: vi.fn(), stopPropagation: vi.fn() };
		const event = { x: 5, y: 6, original, preventSigmaDefault: vi.fn() };
		instance.emit('downNode', { node: 'b', event, preventSigmaDefault: vi.fn() });
		instance.emit('moveBody', { event, preventSigmaDefault: vi.fn() });
		expect(instance.graph.getNodeAttributes('b')).toMatchObject({ x: 5, y: 6 });
		instance.emit('upNode');
		instance.emit('clickNode', { node: 'b' });
		expect(onSelect).toHaveBeenCalledTimes(1);
	});

	it('draws short and wrapped hover labels', async () => {
		const { instance } = await mounted();
		const settings = { labelWeight: '500', labelSize: 12, labelFont: 'sans-serif' };
		const context = canvasContext();
		const node = instance.graph.getNodeAttributes('a');
		(instance.settings.defaultDrawNodeLabel as LabelDrawer)(context, node, settings);
		(instance.settings.defaultDrawNodeHover as LabelDrawer)(
			context,
			{ ...node, fullLabel: `${'word '.repeat(40)}${'x'.repeat(80)}` },
			settings
		);
		(instance.settings.defaultDrawNodeLabel as LabelDrawer)(
			context,
			{ ...node, label: '' },
			settings
		);
		expect(vi.mocked(context.fillText).mock.calls.length).toBeGreaterThan(2);
	});

	it('resets the camera and tears down on clear', async () => {
		const { renderer, instance } = await mounted();
		await renderer.reset();
		expect(instance.camera.setState).toHaveBeenCalled();
		renderer.refreshAppearance({ colorBy: 'study', fields });
		renderer.destroy();
		expect(instance.kill).toHaveBeenCalled();
	});
});
