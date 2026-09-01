<script lang="ts">
	import type { GraphNodeDto, ProjectGraphDto } from '$lib/api/generated/models';
	import { createGetProjectGraph } from '$lib/api/generated/reports/reports';
	import GraphDegradedState from '$lib/components/GraphDegradedState.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as InputGroup from '$lib/components/ui/input-group';
	import { Slider } from '$lib/components/ui/slider';
	import { Spinner } from '$lib/components/ui/spinner';
	import { PageHeader, PageToolbar, StatePanel, Surface } from '$lib/components/layout';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { onCleanup, watch } from 'runed';
	import { useProjectWorkspaceContext, type GraphOverlayField } from './context.svelte.js';
	import { overlaySummary } from './graph-overlays';
	import {
		createProjectGraphRenderer,
		type ProjectGraphRenderModel
	} from './project-graph-renderer';
	import { activeProjectQuery, createActiveProjectProjection } from './project-queries.svelte';
	import { reportSearchText } from './report-label';

	const FILTER_UPDATE_DELAY_MS = 150;
	const overlayLabels: ReadonlyArray<{ field: GraphOverlayField; label: string }> = [
		{ field: 'metrics', label: 'Metrics' },
		{ field: 'screening', label: 'Screening' },
		{ field: 'study', label: 'Study' },
		{ field: 'appraisal', label: 'Appraisal' },
		{ field: 'provenance', label: 'Provenance' }
	];

	const workspace = useProjectWorkspaceContext();
	let container = $state<HTMLDivElement | null>(null);
	let graphRendering = $state(false);
	let renderVersion = 0;
	const enabled = $derived(workspace.view === 'graph');

	const graphQuery = createGetProjectGraph(
		() => workspace.project.id,
		() => ({ fields: workspace.graphFilters.fields.join(',') }),
		() => activeProjectQuery(workspace.project.id, enabled)
	);
	const projectionQuery = createActiveProjectProjection(
		() => workspace.project.id,
		() => enabled
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

	function isGraphOverlayField(value: string): value is GraphOverlayField {
		return overlayLabels.some(({ field }) => field === value);
	}

	function setColorBy(event: Event) {
		const target = event.currentTarget;
		if (!(target instanceof HTMLSelectElement) || !isGraphOverlayField(target.value)) return;
		workspace.graphFilters.colorBy = target.value;
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
	const visibleNodeIds = $derived(new Set(visibleNodes.map((node) => node.report_id)));

	function renderModel(): ProjectGraphRenderModel {
		return {
			nodes: graphData.nodes,
			edges: graphData.edges,
			visibleNodeIds,
			selectedArticle: workspace.selectedArticle,
			colorBy: workspace.graphFilters.colorBy,
			fields: workspace.graphFilters.fields
		};
	}

	const graphRenderer = createProjectGraphRenderer({
		onSelect: (reportId) => workspace.openArticle(reportId),
		onClear: () => workspace.clearArticle()
	});

	function graphContainerAttachment(element: Element) {
		if (!(element instanceof HTMLDivElement)) {
			throw new TypeError('The graph renderer requires an HTMLDivElement');
		}
		container = element;
		graphRenderer.mount(element);
		return () => {
			if (container === element) container = null;
		};
	}

	async function renderCurrentGraph(options: { resetCamera?: boolean } = {}) {
		const version = ++renderVersion;
		graphRendering = true;
		try {
			await graphRenderer.update(renderModel(), options);
		} finally {
			if (version === renderVersion) graphRendering = false;
		}
	}

	function resetLayout() {
		if (container && enabled && visibleNodes.length > 0) {
			void renderCurrentGraph({ resetCamera: true });
		}
	}

	watch(
		[() => enabled, () => container, () => graphData.nodes, () => graphData.edges],
		([active, target, nodes]) => {
			if (!active || !target || nodes.length === 0) {
				renderVersion += 1;
				graphRenderer.clear();
				graphRendering = false;
				return;
			}
			void renderCurrentGraph();
		}
	);

	watch([() => enabled, () => visibleNodeIds], ([active, nextVisibleNodeIds]) => {
		if (!active) return;
		const timeout = setTimeout(
			() => graphRenderer.setVisibleNodes(nextVisibleNodeIds),
			FILTER_UPDATE_DELAY_MS
		);
		return () => clearTimeout(timeout);
	});

	watch([() => enabled, () => workspace.selectedArticle], ([active, selectedArticle]) => {
		if (active) graphRenderer.setSelection(selectedArticle);
	});

	watch(
		[
			() => enabled,
			() => workspace.graphFilters.colorBy,
			() => workspace.graphFilters.fields.join(',')
		],
		([active]) => {
			if (!active) return;
			graphRenderer.refreshAppearance({
				colorBy: workspace.graphFilters.colorBy,
				fields: workspace.graphFilters.fields
			});
		}
	);

	onCleanup(() => graphRenderer.destroy());
</script>

<div
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	tabindex="-1"
	data-testid="graph-page"
>
	<div
		class="mx-auto flex w-full max-w-[1440px] flex-1 flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8"
	>
		<PageHeader
			eyebrow="Evidence workspace / Analysis"
			title="Graph"
			description="Project-local citation network, directed from citing work to cited work."
		/>

		{#if graphQuery.data}
			<PageToolbar label="Graph projection status">
				<div class="flex flex-wrap items-center gap-2" data-testid="projection-metadata">
					<Badge variant="secondary"
						>Projection revision {graphData.projection.revision}</Badge
					>
					<Badge variant="outline">Lag {graphData.projection.lag}</Badge>
					{#if graphData.projection.last_success_at}
						<Badge variant="outline">
							Projected {new Date(
								graphData.projection.last_success_at
							).toLocaleString()}
						</Badge>
					{/if}
					{#if graphData.truncated}<Badge variant="secondary">Bounded result</Badge>{/if}
				</div>
			</PageToolbar>
		{/if}

		<PageToolbar label="Graph controls" class="items-stretch">
			<div class="grid w-full gap-3 md:grid-cols-[minmax(0,1fr)_minmax(14rem,280px)_auto]">
				<InputGroup.Root>
					<InputGroup.Input
						placeholder="Search node label"
						aria-label="Search graph nodes"
						bind:value={workspace.graphFilters.search}
					/>
					<InputGroup.Addon><SearchIcon data-icon /></InputGroup.Addon>
				</InputGroup.Root>
				<div class="flex items-center gap-3">
					<Slider
						type="single"
						bind:value={workspace.graphFilters.minInternal}
						max={20}
						step={1}
						disabled={!overlayEnabled('metrics')}
						thumbLabel="Minimum internal citations"
					/>
					<Badge variant="outline" class="shrink-0">
						{overlayEnabled('metrics')
							? `Internal ${workspace.graphFilters.minInternal}+`
							: 'Internal filter unavailable'}
					</Badge>
				</div>
				<Badge variant="secondary" class="w-fit self-center">
					{graphData.nodes.length} nodes and {graphData.edges.length} edges
				</Badge>
			</div>
		</PageToolbar>

		<fieldset
			class="rounded-lg bg-card p-4 ring-1 ring-foreground/10"
			data-testid="graph-overlay-filters"
		>
			<legend class="px-1 text-sm font-semibold">Visual overlays</legend>
			<div class="mt-2 flex flex-wrap items-center gap-x-4 gap-y-2">
				{#each overlayLabels as { field, label } (field)}
					<label class="flex min-h-9 items-center gap-2 text-sm text-muted-foreground">
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
				<label class="flex min-h-9 items-center gap-2 text-sm md:ml-auto">
					<span class="text-muted-foreground">Color by</span>
					<select
						aria-label="Color graph by"
						class="h-9 min-w-40 rounded-md border border-input bg-background px-3 text-sm shadow-xs outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
						value={workspace.graphFilters.colorBy}
						onchange={setColorBy}
					>
						{#each overlayLabels as { field, label } (field)}
							<option value={field} disabled={!overlayEnabled(field)}>{label}</option>
						{/each}
					</select>
				</label>
			</div>
		</fieldset>

		<div class="grid gap-4 md:grid-cols-2" data-testid="graph-overlay-legend">
			{#if !overlayEnabled(workspace.graphFilters.colorBy)}
				<Surface as="section" tone="subtle" class="p-4 text-sm text-muted-foreground">
					Color-by overlay is not loaded. Select it above or enable its field.
				</Surface>
			{:else if workspace.graphFilters.colorBy === 'screening'}
				<Surface
					as="section"
					tone="subtle"
					class="p-4 text-sm"
					label="Screening overlay legend"
				>
					<h2 class="font-semibold">Screening overlay</h2>
					<div class="mt-3 flex flex-wrap gap-x-4 gap-y-2 text-sm text-muted-foreground">
						<span class="legend-item"
							><span class="legend-swatch screening-include" aria-hidden="true"
							></span>include</span
						>
						<span class="legend-item"
							><span class="legend-swatch screening-exclude" aria-hidden="true"
							></span>exclude</span
						>
						<span class="legend-item"
							><span class="legend-swatch screening-pending" aria-hidden="true"
							></span>pending / unscreened</span
						>
					</div>
				</Surface>
			{:else if workspace.graphFilters.colorBy === 'metrics'}
				<Surface
					as="section"
					tone="subtle"
					class="p-4 text-sm"
					label="Metrics overlay legend"
				>
					<h2 class="font-semibold">Metrics overlay</h2>
					<div class="mt-3 flex flex-wrap gap-x-4 gap-y-2 text-sm text-muted-foreground">
						<span class="legend-item"
							><span class="legend-swatch metrics-cited" aria-hidden="true"
							></span>internally cited</span
						>
						<span class="legend-item"
							><span class="legend-swatch metrics-uncited" aria-hidden="true"
							></span>no internal citations</span
						>
					</div>
					<p class="mt-2 text-xs text-muted-foreground">
						Rank and citation counts are available when a node is selected.
					</p>
				</Surface>
			{:else}
				<Surface
					as="section"
					tone="subtle"
					class="p-4 text-sm"
					label="Evidence overlay legend"
				>
					<h2 class="font-semibold">Evidence overlays</h2>
					<div class="mt-3 flex flex-wrap gap-x-4 gap-y-2 text-sm text-muted-foreground">
						{#if workspace.graphFilters.colorBy === 'study'}<span class="legend-item"
								><span class="legend-swatch evidence-grouped" aria-hidden="true"
								></span>grouped</span
							><span class="legend-item"
								><span class="legend-swatch evidence-ungrouped" aria-hidden="true"
								></span>ungrouped</span
							>{/if}
						{#if workspace.graphFilters.colorBy === 'appraisal'}<span
								class="legend-item"
								><span class="legend-swatch evidence-appraised" aria-hidden="true"
								></span>appraised</span
							><span class="legend-item"
								><span
									class="legend-swatch evidence-not-appraised"
									aria-hidden="true"
								></span>not appraised</span
							>{/if}
						{#if workspace.graphFilters.colorBy === 'provenance'}<span
								class="legend-item"
								><span class="legend-swatch evidence-acquired" aria-hidden="true"
								></span>acquired</span
							><span class="legend-item"
								><span class="legend-swatch evidence-no-source" aria-hidden="true"
								></span>no source</span
							>{/if}
					</div>
				</Surface>
			{/if}
		</div>

		{#if selectedGraphNode}
			<Surface as="aside" tone="inset" class="p-4" label="Selected node overlay summary">
				<h2 class="text-sm font-semibold">Selected node overlays</h2>
				<div class="mt-2 grid gap-1 text-sm text-muted-foreground sm:grid-cols-2">
					{#each overlaySummary(selectedGraphNode, workspace.graphFilters.fields) as summary (summary)}
						<div>{summary}</div>
					{/each}
				</div>
			</Surface>
		{/if}

		{#if graphQuery.error}
			<GraphDegradedState
				error={graphQuery.error}
				projection={projectionQuery.data?.data}
				onRetry={() => void graphQuery.refetch()}
			/>
		{:else if graphQuery.isPending && enabled}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="loading"
					title="Preparing network"
					description="Computing a stable layout from the current overlay selection."
				/>
			</Surface>
		{:else if graphData.nodes.length === 0 || visibleNodes.length === 0}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="empty"
					title="Empty graph"
					description="No graph nodes match this project and filter set."
				/>
			</Surface>
		{:else}
			<div class="min-h-0 min-w-0 flex-1">
				<div
					class="graph-frame relative h-full min-h-[520px] overflow-hidden rounded-lg ring-1 ring-foreground/10"
					aria-label="Graph canvas"
					aria-busy={graphRendering}
				>
					<div {@attach graphContainerAttachment} class="absolute inset-0"></div>
					{#if graphRendering}
						<div
							class="absolute inset-0 z-20 flex items-center justify-center bg-background/70 backdrop-blur-[1px]"
							role="status"
						>
							<div
								class="flex items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm shadow-sm"
							>
								<Spinner class="size-4" aria-hidden="true" />
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
						<RotateCcwIcon data-icon aria-hidden="true" />
					</Button>
				</div>
			</div>
		{/if}
	</div>
</div>

<style>
	.graph-frame {
		--graph-bg: var(--background);
		--graph-node-default: var(--muted-foreground);
		--graph-node-cited: var(--chart-4);
		--graph-node-selected: var(--chart-2);
		--graph-node-focused: var(--chart-1);
		--graph-node-dimmed: var(--muted-foreground);
		--graph-edge-default: var(--border);
		--graph-edge-focused: var(--chart-4);
		--graph-edge-dimmed: var(--muted-foreground);
		--graph-label: var(--foreground);
		--graph-label-muted: var(--muted-foreground);
		--graph-label-halo: var(--background);
		--graph-hover-background: var(--card);
		background: var(--graph-bg);
		cursor: grab;
	}

	.graph-frame:global(.graph-dragging) {
		cursor: grabbing;
	}

	.graph-frame :global(canvas) {
		cursor: inherit;
	}

	.legend-item {
		display: inline-flex;
		align-items: center;
		gap: 0.4rem;
	}

	.legend-swatch {
		display: inline-block;
		height: 0.65rem;
		width: 0.65rem;
		border-radius: 999px;
		background: currentColor;
		box-shadow: 0 0 0 2px color-mix(in oklch, currentColor 18%, transparent);
	}

	.screening-include,
	.evidence-appraised {
		color: var(--chart-4);
	}

	.screening-exclude {
		color: var(--chart-2);
	}

	.screening-pending {
		color: var(--chart-1);
	}

	.metrics-cited {
		color: var(--chart-4);
	}

	.metrics-uncited {
		color: var(--muted-foreground);
	}

	.evidence-grouped,
	.evidence-acquired {
		color: var(--chart-1);
	}

	.evidence-ungrouped,
	.evidence-no-source {
		color: var(--muted-foreground);
	}

	.evidence-not-appraised {
		color: var(--muted-foreground);
	}
</style>
