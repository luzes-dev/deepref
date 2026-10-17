import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { GraphEdgeDto, GraphNodeDto } from '#lib/api/generated/models/index.js';
import type { GraphOverlayField } from './context.svelte.js';
import { createProjectGraphRenderer, type ProjectGraphRenderModel } from './project-graph-renderer';

type Attributes = Record<string, unknown>;
type Reducer = (key: string, data: Attributes, attrs: Attributes) => Attributes;
type Handler = (payload: Record<string, unknown>) => void;
type FakeGraph = import('graphology').default<Attributes, Attributes>;

const sigma = vi.hoisted(() => ({ instances: [] as FakeSigma[] }));

class FakeSigma {
	graph: FakeGraph;
	options: {
		settings: Record<string, unknown>;
		nodeReducer?: Reducer;
		edgeReducer?: Reducer;
	};
	handlers = new Map<string, Handler>();
	refresh = vi.fn();
	setCustomBBox = vi.fn();
	setSetting = vi.fn();
	kill = vi.fn();
	camera = { setState: vi.fn() };

	constructor(graph: FakeGraph, _target: HTMLElement, options: FakeSigma['options']) {
		this.graph = graph;
		this.options = options;
		sigma.instances.push(this);
	}
	getGraph() {
		return this.graph;
	}
	setGraph(graph: FakeGraph) {
		this.graph = graph;
	}
	setSettings(settings: Record<string, unknown>) {
		this.options.settings = { ...this.options.settings, ...settings };
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
	graphToViewport({ x, y }: { x: number; y: number }) {
		return { x, y };
	}
	scaleSize(size: number) {
		return size;
	}
	getNodeDisplayData(key: string) {
		return { ...this.graph.getNodeAttributes(key), ...this.node(key) };
	}
	getGraphState() {
		return { isIdle: true };
	}
	on(event: string, handler: Handler) {
		this.handlers.set(event, handler);
	}
	emit(event: string, payload: Record<string, unknown> = {}) {
		this.handlers.get(event)?.(payload);
	}
	node(key: string): Attributes {
		const attrs = this.graph.getNodeAttributes(key);
		return (this.options.nodeReducer as Reducer)(key, attrs, attrs);
	}
	edge(key: string): Attributes {
		const attrs = this.graph.getEdgeAttributes(key);
		return (this.options.edgeReducer as Reducer)(key, attrs, attrs);
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
		expect(instance.node('c').visibility).toBe('hidden');
		expect(instance.node('a').visibility).toBe('visible');
		const [ab, bc] = instance.graph.edges();
		expect(instance.edge(ab!).visibility).toBe('visible');
		expect(instance.edge(bc!).visibility).toBe('hidden');
	});

	it('focuses the hovered node and its neighbours and dims the rest', async () => {
		const { instance } = await mounted();
		instance.emit('enterNode', { node: 'a' });
		expect(instance.node('a').labelVisibility).toBe('hidden');
		expect(instance.node('b').labelVisibility).toBe('visible');
		expect(instance.node('d').labelVisibility).toBe('hidden');
		const [ab, bc] = instance.graph.edges();
		expect(instance.edge(ab!).size).toBe(2);
		expect(instance.edge(bc!).size).toBe(0.5);
		instance.emit('leaveNode');
		// Small graphs keep forced labels (v3's `forceLabel` behavior).
		expect(instance.node('d').labelVisibility).toBe('visible');
	});

	it('replaces the hovered native label with one full-title card', async () => {
		const { renderer, instance } = await mounted();
		instance.emit('enterNode', { node: 'a' });
		instance.emit('afterRender');
		const visibleCards = () => document.querySelectorAll('.graph-hover-card:not([hidden])');
		expect(visibleCards()).toHaveLength(1);
		expect(visibleCards()[0]?.textContent).toBe(nodes[0].title);
		expect(instance.node('a').labelVisibility).toBe('hidden');

		renderer.setSelection('a');
		instance.emit('afterRender');
		expect(visibleCards()).toHaveLength(1);
		instance.emit('leaveNode');
		instance.emit('afterRender');
		expect(visibleCards()).toHaveLength(1);
		expect(visibleCards()[0]?.textContent).toBe(nodes[0].title);

		renderer.setSelection(undefined);
		instance.emit('afterRender');
		expect(visibleCards()).toHaveLength(0);
		expect(instance.node('a').labelVisibility).toBe('visible');
		renderer.destroy();
	});

	it('hides filtered cards and removes them when the graph is cleared', async () => {
		const { renderer, instance } = await mounted();
		renderer.setSelection('b');
		instance.emit('afterRender');
		expect(document.querySelector('.graph-hover-card:not([hidden])')?.textContent).toBe(
			nodes[1].title
		);
		expect(instance.node('b').labelVisibility).toBe('hidden');
		expect(instance.node('a').labelVisibility).toBe('visible');
		expect(instance.node('d').labelVisibility).toBe('hidden');

		renderer.setVisibleNodes(new Set(['a', 'c', 'd']));
		instance.emit('afterRender');
		expect(document.querySelector('.graph-hover-card:not([hidden])')).toBeNull();
		renderer.clear();
		expect(document.querySelector('.graph-hover-card')).toBeNull();
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
});
